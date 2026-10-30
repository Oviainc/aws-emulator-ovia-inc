// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the workflow (flow template) slice over the service's real
//! `awsJson1_1` wire: `POST /` with
//! `X-Amz-Target: IotThingsGraphFrontEndService.<OperationName>`.
//!
//! Every request goes through the same `MultiServiceRouter` the production entrypoint
//! serves, so deserialization, modeled-constraint validation, the handler and
//! serialization all run exactly as they do for a real SDK client.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotthingsgraph_iotthingsgraphfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The default region these tests sign for.
const REGION: &str = "us-east-1";

/// A router carrying only this service, over its own fresh engine.
fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests sign nothing; the `Authorization` header below exists to carry the
    // routing key and the region, not a real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Invoke one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, region: &str, target: &str, body: Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/iotthingsgraph/aws4_request, \
         SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iotthingsgraph.example.com/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("IotThingsGraphFrontEndService.{target}"),
        )
        .header("authorization", authorization)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "response to {target} is not JSON ({error}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// Invoke one operation in [`REGION`].
async fn invoke(router: &MultiServiceRouter, target: &str, body: Value) -> (u16, Value) {
    call(router, REGION, target, body).await
}

/// The wire error code of an error body (`__type`, less any namespace prefix).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The id a workflow called `name` carries in `region`, written out here rather than
/// taken from the implementation: `urn:tdm:<region>/<account>/default:workflow:<name>`
/// is the form the modelled `Urn` pattern describes.
fn expected_workflow_id(region: &str, name: &str) -> String {
    format!(
        "urn:tdm:{region}/{}/default:workflow:{name}",
        engine::emulator_account_id()
    )
}

/// The ARN a workflow called `name` carries in `region`.
fn expected_workflow_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:iotthingsgraph:{region}:{}:Workflow/default/{name}",
        engine::emulator_account_id()
    )
}

/// A `DefinitionDocument` request member around `text`.
fn definition(text: &str) -> Value {
    json!({"language": "GRAPHQL", "text": text})
}

/// A string member of a JSON object, named in the panic when it is absent.
fn string_at(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer} in {body}"))
        .to_string()
}

/// An integer member of a JSON object, named in the panic when it is absent.
fn i64_at(case: &str, body: &Value, pointer: &str) -> i64 {
    body.pointer(pointer)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an integer at {pointer} in {body}"))
}

/// The ids a `summaries` list reports, sorted so a set comparison does not depend on the
/// page order.
fn summary_ids(case: &str, body: &Value) -> Vec<String> {
    let summaries = body
        .get("summaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a `summaries` array in {body}"));
    let mut ids: Vec<String> = summaries
        .iter()
        .map(|summary| string_at(case, summary, "/id"))
        .collect();
    ids.sort();
    ids
}

/// Walk every page of a paginated operation at `page_size`, returning each summary in the
/// order the pages reported them.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    target: &str,
    request: &Value,
    page_size: Option<i32>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    // One more page than there could ever be records, so a token that never clears is a
    // failed assertion rather than a hanging test.
    for _ in 0..512 {
        let mut body = request.clone();
        if let Some(size) = page_size {
            body["maxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let (status, response) = invoke(router, target, body).await;
        assert_eq!(status, 200, "case {case:?}: {target} page: {response}");
        let page = response
            .get("summaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected `summaries` in {response}"));
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "case {case:?}: a page of {} summaries exceeds maxResults {size}",
                page.len()
            );
        }
        collected.extend(page.iter().cloned());
        next_token = response
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: {target} never stopped handing back a nextToken");
}

/// One row of the definition-document table: a workflow name and the GraphQL document
/// that defines it.
struct DefinitionCase {
    case: &'static str,
    name: &'static str,
    text: String,
}

/// Documents spanning what a definition can look like: a bare selection, variables,
/// directives, nesting, entity references, comments and odd whitespace, and a workflow
/// name at the longest the derived id can carry.
fn definition_cases() -> Vec<DefinitionCase> {
    // The modelled `Urn` is `@length(0..160)`, and
    // `urn:tdm:us-east-1/000000000000/default:workflow:` is 48 of those characters.
    let longest_name = "L".repeat(160 - expected_workflow_id(REGION, "").len());
    vec![
        DefinitionCase {
            case: "bare selection",
            name: "SimpleFlow",
            text: "query SimpleFlow { steps }".to_string(),
        },
        DefinitionCase {
            case: "variables and nesting",
            name: "NestedFlow",
            text: "query NestedFlow($thing: String!) { variables { lamp } steps { s1 { s2 } } }"
                .to_string(),
        },
        DefinitionCase {
            case: "directive and an entity reference",
            name: "Flow_3",
            text: "query Flow_3 @workflowType(value: \"FLOW\") {\n  steps {\n    trigger(device: \
                   \"urn:tdm:aws/examples:deviceModel:MotionSensor\")\n  }\n}"
                .to_string(),
        },
        DefinitionCase {
            case: "comments and ragged whitespace",
            name: "CommentedFlow",
            text: "# a workflow\n\n  query    CommentedFlow   {\n\t steps \n }\n\n".to_string(),
        },
        DefinitionCase {
            case: "longest name the derived id can carry",
            name: Box::leak(longest_name.clone().into_boxed_str()),
            text: format!("query {longest_name} {{ steps }}"),
        },
    ]
}

/// RULE: a workflow's id is derived from the workflow named by its definition document
/// and from the Region the request was signed for; the definition itself is stored
/// verbatim, and every revision stays readable at its own `revisionNumber` after a later
/// one is appended.
#[tokio::test]
async fn a_workflow_round_trips_through_every_revision_of_its_definition() {
    let router = new_router();

    for DefinitionCase { case, name, text } in definition_cases() {
        let (status, created) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&text), "compatibleNamespaceVersion": 7}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateFlowTemplate: {created}");

        let id = expected_workflow_id(REGION, name);
        assert_eq!(
            string_at(case, &created, "/summary/id"),
            id,
            "case {case:?}: the minted id must name the workflow the definition defines, got {created}"
        );
        assert_eq!(
            string_at(case, &created, "/summary/arn"),
            expected_workflow_arn(REGION, name),
            "case {case:?}: unexpected ARN in {created}"
        );
        assert_eq!(
            i64_at(case, &created, "/summary/revisionNumber"),
            1,
            "case {case:?}: a created workflow is revision 1, got {created}"
        );
        assert!(
            created
                .pointer("/summary/createdAt")
                .and_then(Value::as_f64)
                .is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected a createdAt timestamp in {created}"
        );

        // The stored document comes back byte-for-byte, under the language it was
        // uploaded with and the namespace version the request asked to be compatible
        // with.
        let (status, fetched) =
            invoke(&router, "GetFlowTemplate", json!({"id": id.clone()})).await;
        assert_eq!(status, 200, "case {case:?}: GetFlowTemplate: {fetched}");
        assert_eq!(
            string_at(case, &fetched, "/description/definition/text"),
            text,
            "case {case:?}: the stored definition text must be the uploaded one"
        );
        assert_eq!(
            string_at(case, &fetched, "/description/definition/language"),
            "GRAPHQL",
            "case {case:?}: unexpected definition language in {fetched}"
        );
        assert_eq!(
            i64_at(case, &fetched, "/description/validatedNamespaceVersion"),
            7,
            "case {case:?}: unexpected validatedNamespaceVersion in {fetched}"
        );

        // A read must not change what the next caller sees.
        let (_, fetched_again) = invoke(&router, "GetFlowTemplate", json!({"id": id.clone()})).await;
        assert_eq!(
            fetched, fetched_again,
            "case {case:?}: two reads of one workflow must answer identically"
        );

        // An update appends a revision; it does not rewrite the one before it.
        let updated_text = format!("{text}\n# revision 2\n");
        let (status, updated) = invoke(
            &router,
            "UpdateFlowTemplate",
            json!({"id": id.clone(), "definition": definition(&updated_text)}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: UpdateFlowTemplate: {updated}");
        assert_eq!(
            i64_at(case, &updated, "/summary/revisionNumber"),
            2,
            "case {case:?}: an update appends the next revision, got {updated}"
        );
        assert_eq!(
            string_at(case, &updated, "/summary/id"),
            id,
            "case {case:?}: an update keeps the workflow's id, got {updated}"
        );

        let (_, latest) = invoke(&router, "GetFlowTemplate", json!({"id": id.clone()})).await;
        assert_eq!(
            string_at(case, &latest, "/description/definition/text"),
            updated_text,
            "case {case:?}: a read with no revisionNumber must answer from the newest revision"
        );
        assert_eq!(
            i64_at(case, &latest, "/description/summary/revisionNumber"),
            2,
            "case {case:?}: unexpected revisionNumber in {latest}"
        );
        assert!(
            latest.pointer("/description/validatedNamespaceVersion").is_none(),
            "case {case:?}: the update named no compatibleNamespaceVersion, so the new \
             revision was validated against none: {latest}"
        );

        let (_, first) = invoke(
            &router,
            "GetFlowTemplate",
            json!({"id": id.clone(), "revisionNumber": 1}),
        )
        .await;
        assert_eq!(
            string_at(case, &first, "/description/definition/text"),
            text,
            "case {case:?}: revision 1 must still answer with the definition it stored"
        );

        // Every revision is listed, oldest first.
        let revisions = walk_pages(
            &router,
            case,
            "GetFlowTemplateRevisions",
            &json!({"id": id.clone()}),
            None,
        )
        .await;
        let numbers: Vec<i64> = revisions
            .iter()
            .map(|summary| i64_at(case, summary, "/revisionNumber"))
            .collect();
        assert_eq!(
            numbers,
            vec![1, 2],
            "case {case:?}: GetFlowTemplateRevisions must report every stored revision"
        );
    }
}

/// RULE: the Region a request is signed for is part of the workflow id it mints, so the
/// same definition creates a distinct workflow per Region.
#[tokio::test]
async fn the_signing_region_is_part_of_the_minted_workflow_id() {
    let router = new_router();
    let text = "query RegionalFlow { steps }";

    // Three Region spellings the modelled `Urn` pattern admits for the id's namespace.
    let mut minted = Vec::new();
    for region in ["us-east-1", "eu-west-2", "ap-southeast-3"] {
        let (status, created) = call(
            &router,
            region,
            "CreateFlowTemplate",
            json!({"definition": definition(text)}),
        )
        .await;
        assert_eq!(status, 200, "case {region:?}: CreateFlowTemplate: {created}");
        assert_eq!(
            string_at(region, &created, "/summary/id"),
            expected_workflow_id(region, "RegionalFlow"),
            "case {region:?}: the id must carry the Region the request was signed for"
        );
        assert_eq!(
            string_at(region, &created, "/summary/arn"),
            expected_workflow_arn(region, "RegionalFlow"),
            "case {region:?}: the ARN must carry the Region the request was signed for"
        );
        minted.push(string_at(region, &created, "/summary/id"));
    }

    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "one definition signed for three Regions must mint three distinct ids, got {minted:?}"
    );
}

/// RULE: a definition document that does not name exactly one workflow, or that the
/// GraphQL parser cannot read within this emulator's bounds, is refused with
/// `InvalidRequestException` — and nothing is stored.
#[tokio::test]
async fn a_definition_that_names_no_single_workflow_is_refused() {
    let router = new_router();

    // 160 is the modelled `@length` maximum on `Urn`, so one character more than the
    // longest name that fits is the first that cannot be stored.
    let too_long = "L".repeat(160 - expected_workflow_id(REGION, "").len() + 1);
    let past_recursion_limit = format!(
        "query Deep {}{}",
        "{ a ".repeat(200),
        "}".repeat(200)
    );
    let past_token_limit = format!("query Wide {{ {} }}", "a ".repeat(100_001));

    let cases: Vec<(&str, String)> = vec![
        ("empty text", String::new()),
        ("not GraphQL at all", "this is not a workflow".to_string()),
        ("unterminated selection", "query Truncated {".to_string()),
        ("anonymous operation", "query { steps }".to_string()),
        (
            "two operations",
            "query FlowA { steps } query FlowB { steps }".to_string(),
        ),
        (
            "no operation at all",
            "fragment Parts on Step { name }".to_string(),
        ),
        ("name past the id length bound", format!("query {too_long} {{ steps }}")),
        ("nesting past the parser's recursion bound", past_recursion_limit),
        ("more tokens than the parser reads", past_token_limit),
    ];

    for (case, text) in cases {
        let (status, body) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&text)}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // Nothing above was stored.
    let (status, listed) = invoke(&router, "SearchFlowTemplates", json!({})).await;
    assert_eq!(status, 200, "SearchFlowTemplates: {listed}");
    assert_eq!(
        summary_ids("refused definitions", &listed),
        Vec::<String>::new(),
        "a refused CreateFlowTemplate must store nothing: {listed}"
    );
}

/// RULE: a value outside a constraint the model declares is refused with this service's
/// `InvalidRequestException` (HTTP 400), whichever member carries the constraint.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused() {
    let router = new_router();

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "absent required definition",
            "CreateFlowTemplate",
            json!({}),
        ),
        (
            "language outside the DefinitionLanguage enum",
            "CreateFlowTemplate",
            json!({"definition": {"language": "PYTHON", "text": "query F { steps }"}}),
        ),
        (
            "absent required id",
            "GetFlowTemplate",
            json!({}),
        ),
        (
            "id outside the Urn pattern",
            "GetFlowTemplate",
            json!({"id": "not-a-urn"}),
        ),
        (
            "maxResults below the modelled range",
            "SearchFlowTemplates",
            json!({"maxResults": 0}),
        ),
        (
            "maxResults above the modelled range",
            "SearchFlowTemplates",
            json!({"maxResults": 251}),
        ),
        (
            "revision listing maxResults above the modelled range",
            "GetFlowTemplateRevisions",
            json!({"id": expected_workflow_id(REGION, "Missing"), "maxResults": 251}),
        ),
    ];

    for (case, target, body) in cases {
        let (status, response) = invoke(&router, target, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {target}, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {case:?}: unexpected error code from {target} in {response}"
        );
    }

    // Both published bounds of `maxResults` are accepted.
    for page_size in [1, 250] {
        let (status, response) =
            invoke(&router, "SearchFlowTemplates", json!({"maxResults": page_size})).await;
        assert_eq!(
            status, 200,
            "case {page_size:?}: a maxResults on the modelled bound must be accepted: {response}"
        );
    }
}

/// RULE: an id that names no workflow is a bad request, a workflow id that names nothing
/// stored is not found, and a second create of a workflow that exists conflicts.
#[tokio::test]
async fn an_id_naming_no_stored_workflow_is_refused_per_operation() {
    let router = new_router();

    // Ids that are not a workflow URN at all. Each is refused by every operation that
    // takes an id, whether the modelled pattern catches it at the boundary or the handler
    // finds it names another kind of entity.
    let not_workflow_ids = [
        "not-a-urn",
        "urn:tdm:",
        "urn:tdm:aws/examples:deviceModel:Camera",
        "urn:tdm:us-east-1/000000000000/default:device:Lamp",
    ];
    for id in not_workflow_ids {
        for target in [
            "GetFlowTemplate",
            "GetFlowTemplateRevisions",
            "DeleteFlowTemplate",
        ] {
            let (status, body) = invoke(&router, target, json!({"id": id})).await;
            assert_eq!(
                status, 400,
                "case ({id:?}, {target:?}): expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case ({id:?}, {target:?}): unexpected error code in {body}"
            );
        }
        let (status, body) = invoke(
            &router,
            "UpdateFlowTemplate",
            json!({"id": id, "definition": definition("query F { steps }")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case ({id:?}, \"UpdateFlowTemplate\"): expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case ({id:?}, \"UpdateFlowTemplate\"): unexpected error code in {body}"
        );
    }

    // A well-formed workflow id that names nothing stored is NOT FOUND, not invalid.
    let absent = expected_workflow_id(REGION, "NeverCreated");
    for target in ["GetFlowTemplate", "GetFlowTemplateRevisions"] {
        let (status, body) = invoke(&router, target, json!({"id": absent.clone()})).await;
        assert_eq!(
            status, 404,
            "case {target:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {target:?}: unexpected error code in {body}"
        );
    }
    let (status, body) = invoke(
        &router,
        "UpdateFlowTemplate",
        json!({"id": absent.clone(), "definition": definition("query NeverCreated { steps }")}),
    )
    .await;
    assert_eq!(status, 404, "UpdateFlowTemplate on an absent workflow: {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unexpected error code in {body}"
    );

    // A create of a workflow that already exists conflicts, and leaves the stored
    // definition alone.
    let original = "query Conflicting { steps }";
    let (status, created) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition(original)}),
    )
    .await;
    assert_eq!(status, 200, "CreateFlowTemplate: {created}");
    let id = expected_workflow_id(REGION, "Conflicting");

    let (status, conflict) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition("query Conflicting { steps { other } }")}),
    )
    .await;
    assert_eq!(status, 409, "a second create of one workflow: {conflict}");
    assert_eq!(
        error_code(&conflict),
        "ResourceAlreadyExistsException",
        "unexpected error code in {conflict}"
    );
    let (_, fetched) = invoke(&router, "GetFlowTemplate", json!({"id": id.clone()})).await;
    assert_eq!(
        string_at("conflict", &fetched, "/description/definition/text"),
        original,
        "a refused create must not overwrite the stored definition: {fetched}"
    );

    // An update whose definition names a different workflow is not a revision of this
    // one.
    let (status, mismatched) = invoke(
        &router,
        "UpdateFlowTemplate",
        json!({"id": id.clone(), "definition": definition("query SomethingElse { steps }")}),
    )
    .await;
    assert_eq!(status, 400, "an update naming another workflow: {mismatched}");
    assert_eq!(
        error_code(&mismatched),
        "InvalidRequestException",
        "unexpected error code in {mismatched}"
    );
}

/// RULE: deleting a workflow removes it and every revision of it from state, so a later
/// read cannot find it and a search no longer lists it; creating it again afterwards
/// starts a fresh revision 1.
#[tokio::test]
async fn deleting_a_workflow_removes_every_revision_of_it() {
    let router = new_router();
    let text = "query Disposable { steps }";
    let id = expected_workflow_id(REGION, "Disposable");

    let (status, created) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition(text)}),
    )
    .await;
    assert_eq!(status, 200, "CreateFlowTemplate: {created}");
    let (status, updated) = invoke(
        &router,
        "UpdateFlowTemplate",
        json!({"id": id.clone(), "definition": definition("query Disposable { steps { s } }")}),
    )
    .await;
    assert_eq!(status, 200, "UpdateFlowTemplate: {updated}");

    let (status, deleted) = invoke(&router, "DeleteFlowTemplate", json!({"id": id.clone()})).await;
    assert_eq!(status, 200, "DeleteFlowTemplate: {deleted}");

    for revision in [None, Some(1), Some(2)] {
        let mut body = json!({"id": id.clone()});
        if let Some(number) = revision {
            body["revisionNumber"] = json!(number);
        }
        let (status, response) = invoke(&router, "GetFlowTemplate", body).await;
        assert_eq!(
            status, 404,
            "case {revision:?}: a deleted workflow's revisions are gone too: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {revision:?}: unexpected error code in {response}"
        );
    }

    let (_, listed) = invoke(&router, "SearchFlowTemplates", json!({})).await;
    assert_eq!(
        summary_ids("after delete", &listed),
        Vec::<String>::new(),
        "a deleted workflow must not be listed: {listed}"
    );

    // The same definition can be created again, as revision 1 of a new record.
    let (status, recreated) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition(text)}),
    )
    .await;
    assert_eq!(status, 200, "CreateFlowTemplate after delete: {recreated}");
    assert_eq!(
        i64_at("recreated", &recreated, "/summary/revisionNumber"),
        1,
        "a recreated workflow starts at revision 1: {recreated}"
    );
}

/// RULE: a paginated listing hands back every stored record exactly once, in one page per
/// `maxResults`, resuming from the opaque token the previous page returned.
#[tokio::test]
async fn every_page_size_reaches_every_record_exactly_once() {
    let router = new_router();

    // Five workflows, and five revisions of one of them.
    let mut all_ids = Vec::new();
    for index in 0..5 {
        let name = format!("PagedFlow{index}");
        let (status, created) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&format!("query {name} {{ steps }}"))}),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: CreateFlowTemplate: {created}");
        all_ids.push(expected_workflow_id(REGION, &name));
    }
    all_ids.sort();

    let revised = expected_workflow_id(REGION, "PagedFlow0");
    for revision in 2..=5 {
        let (status, updated) = invoke(
            &router,
            "UpdateFlowTemplate",
            json!({
                "id": revised.clone(),
                "definition": definition(&format!("query PagedFlow0 {{ steps {{ r{revision} }} }}")),
            }),
        )
        .await;
        assert_eq!(status, 200, "case {revision:?}: UpdateFlowTemplate: {updated}");
    }

    for page_size in [None, Some(1), Some(2), Some(4), Some(5), Some(6), Some(250)] {
        let case = format!("maxResults {page_size:?}");
        let searched = walk_pages(
            &router,
            &case,
            "SearchFlowTemplates",
            &json!({}),
            page_size,
        )
        .await;
        let mut ids: Vec<String> = searched
            .iter()
            .map(|summary| string_at(&case, summary, "/id"))
            .collect();
        let distinct: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {case:?}: a record was reported twice: {ids:?}"
        );
        ids.sort();
        assert_eq!(
            ids, all_ids,
            "case {case:?}: the pages must reach every stored workflow"
        );

        let revisions = walk_pages(
            &router,
            &case,
            "GetFlowTemplateRevisions",
            &json!({"id": revised.clone()}),
            page_size,
        )
        .await;
        let numbers: Vec<i64> = revisions
            .iter()
            .map(|summary| i64_at(&case, summary, "/revisionNumber"))
            .collect();
        assert_eq!(
            numbers,
            vec![1, 2, 3, 4, 5],
            "case {case:?}: the pages must reach every stored revision, oldest first"
        );
    }
}

/// RULE: a `nextToken` continues the ONE enumeration that minted it; a token this service
/// did not mint, or one handed back to a different search or a different workflow, is
/// refused rather than silently answering a different question.
#[tokio::test]
async fn a_token_from_another_enumeration_is_refused() {
    let router = new_router();
    for name in ["TokenFlowA", "TokenFlowB"] {
        let (status, created) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&format!(
                "query {name} {{ steps(device: \"urn:tdm:aws/examples:deviceModel:Camera\") }}"
            ))}),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: CreateFlowTemplate: {created}");
        let id = expected_workflow_id(REGION, name);
        let (status, updated) = invoke(
            &router,
            "UpdateFlowTemplate",
            json!({"id": id, "definition": definition(&format!("query {name} {{ steps }}"))}),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: UpdateFlowTemplate: {updated}");
    }

    let (_, unfiltered) = invoke(&router, "SearchFlowTemplates", json!({"maxResults": 1})).await;
    let search_token = string_at("unfiltered search", &unfiltered, "/nextToken");

    let (_, revisions) = invoke(
        &router,
        "GetFlowTemplateRevisions",
        json!({"id": expected_workflow_id(REGION, "TokenFlowA"), "maxResults": 1}),
    )
    .await;
    let revision_token = string_at("revisions of A", &revisions, "/nextToken");

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a token this service never minted",
            "SearchFlowTemplates",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a search token handed back with a filter the page was not taken under",
            "SearchFlowTemplates",
            json!({
                "nextToken": search_token.clone(),
                "filters": [{"name": "DEVICE_MODEL_ID",
                             "value": ["urn:tdm:aws/examples:deviceModel:Camera"]}],
            }),
        ),
        (
            "a revision token handed to the template search",
            "SearchFlowTemplates",
            json!({"nextToken": revision_token.clone()}),
        ),
        (
            "a search token handed to a revision listing",
            "GetFlowTemplateRevisions",
            json!({"id": expected_workflow_id(REGION, "TokenFlowA"), "nextToken": search_token}),
        ),
        (
            "another workflow's revision token",
            "GetFlowTemplateRevisions",
            json!({"id": expected_workflow_id(REGION, "TokenFlowB"),
                   "nextToken": revision_token}),
        ),
    ];

    for (case, target, body) in cases {
        let (status, response) = invoke(&router, target, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {target}, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {case:?}: unexpected error code from {target} in {response}"
        );
    }
}

/// RULE: a request that would grow this emulator's store past the ceiling it puts on
/// workflows, on one workflow's revisions, or on the definition bytes it holds, is
/// refused rather than stored — a client loop cannot grow the store without limit, and
/// deleting a workflow gives its bytes back.
#[tokio::test]
async fn the_store_ceilings_refuse_a_further_workflow_and_a_further_revision() {
    use service_com_amazonaws_iotthingsgraph_iotthingsgraphfrontendservice::state::{
        MAX_FLOW_TEMPLATES, MAX_FLOW_TEMPLATE_REVISIONS, MAX_STORED_DEFINITION_BYTES,
    };

    let router = new_router();
    for index in 0..MAX_FLOW_TEMPLATES {
        let (status, created) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&format!("query BulkFlow{index} {{ steps }}"))}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {index:?}: CreateFlowTemplate below the ceiling: {created}"
        );
    }
    let (status, refused) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition("query OneTooMany { steps }")}),
    )
    .await;
    assert_eq!(
        status, 410,
        "case \"one past the workflow ceiling\": expected the modelled quota error, got \
         {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "unexpected error code in {refused}"
    );

    let router = new_router();
    let (status, created) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition("query RevisedFlow { steps }")}),
    )
    .await;
    assert_eq!(status, 200, "CreateFlowTemplate: {created}");
    let id = expected_workflow_id(REGION, "RevisedFlow");
    for revision in 2..=MAX_FLOW_TEMPLATE_REVISIONS {
        let (status, updated) = invoke(
            &router,
            "UpdateFlowTemplate",
            json!({
                "id": id.clone(),
                "definition": definition(&format!("query RevisedFlow {{ steps {{ r{revision} }} }}")),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {revision:?}: UpdateFlowTemplate below the ceiling: {updated}"
        );
    }
    let (status, refused) = invoke(
        &router,
        "UpdateFlowTemplate",
        json!({"id": id.clone(), "definition": definition("query RevisedFlow { steps { last } }")}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the revision ceiling\": expected HTTP 400, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "unexpected error code in {refused}"
    );

    // The byte ceiling: definitions the modelled `DefinitionText` maximum allows, stored
    // until the next one would not fit. Deleting one gives its bytes back, so the create
    // that was refused then succeeds.
    let router = new_router();
    // One definition document just under the 1 MiB `DefinitionText` maximum, carried as a
    // single string value so the parser's token bound is nowhere near reached.
    let one_definition = |index: usize| {
        format!(
            "query HeavyFlow{index} {{ steps(note: \"{}\") }}",
            "x".repeat(1_000_000)
        )
    };
    let mut stored = 0usize;
    let mut refusal = None;
    for index in 0..(MAX_STORED_DEFINITION_BYTES / 1_000_000 + 2) {
        let text = one_definition(index);
        let (status, body) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(&text)}),
        )
        .await;
        if status == 200 {
            stored += text.len();
            continue;
        }
        refusal = Some((index, status, body, text.len()));
        break;
    }
    let (index, status, body, refused_bytes) =
        refusal.expect("a definition must eventually not fit under the byte ceiling");
    assert_eq!(
        status, 410,
        "case {index:?}: expected the modelled quota error, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "case {index:?}: unexpected error code in {body}"
    );
    assert!(
        stored <= MAX_STORED_DEFINITION_BYTES
            && stored + refused_bytes > MAX_STORED_DEFINITION_BYTES,
        "the refusal must come exactly when the next definition no longer fits: {stored} \
         stored, {refused_bytes} refused, ceiling {MAX_STORED_DEFINITION_BYTES}"
    );

    // Deleting a stored workflow releases what it held, so the same create now fits.
    let (delete_status, deleted) = invoke(
        &router,
        "DeleteFlowTemplate",
        json!({"id": expected_workflow_id(REGION, "HeavyFlow0")}),
    )
    .await;
    assert_eq!(delete_status, 200, "DeleteFlowTemplate: {deleted}");
    let (status, accepted) = invoke(
        &router,
        "CreateFlowTemplate",
        json!({"definition": definition(&one_definition(index))}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a delete must give its definition bytes back to the store: {accepted}"
    );
}

/// RULE: `SearchFlowTemplates` narrows its result to the workflows whose CURRENT
/// definition references every device model the filter names; a filter value that names
/// another kind of entity, or a device model nothing references, matches nothing.
#[tokio::test]
async fn the_device_model_filter_narrows_the_search_to_what_references_it() {
    use service_com_amazonaws_iotthingsgraph_iotthingsgraphfrontendservice::state::{
        MAX_SEARCH_FILTERS, MAX_SEARCH_FILTER_VALUES,
    };

    let router = new_router();
    let motion = "urn:tdm:aws/examples:deviceModel:MotionSensor";
    let camera = "urn:tdm:aws/examples:deviceModel:Camera";

    let seeded: Vec<(&str, String)> = vec![
        (
            "FilterFlowMotion",
            format!("query FilterFlowMotion {{ steps(device: \"{motion}\") }}"),
        ),
        (
            "FilterFlowCamera",
            format!("query FilterFlowCamera {{ steps(device: \"{camera}\") }}"),
        ),
        (
            "FilterFlowBoth",
            format!(
                "query FilterFlowBoth {{ steps {{ a(device: \"{motion}\") b(device: \"{camera}\") }} }}"
            ),
        ),
        (
            "FilterFlowNone",
            "query FilterFlowNone { steps }".to_string(),
        ),
    ];
    for (name, text) in &seeded {
        let (status, created) = invoke(
            &router,
            "CreateFlowTemplate",
            json!({"definition": definition(text)}),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: CreateFlowTemplate: {created}");
    }

    let id = |name: &str| expected_workflow_id(REGION, name);
    let a_workflow_id = id("FilterFlowMotion");
    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        (
            "no filter lists every workflow",
            vec![],
            vec![
                id("FilterFlowBoth"),
                id("FilterFlowCamera"),
                id("FilterFlowMotion"),
                id("FilterFlowNone"),
            ],
        ),
        (
            "one device model",
            vec![motion],
            vec![id("FilterFlowBoth"), id("FilterFlowMotion")],
        ),
        (
            "the other device model",
            vec![camera],
            vec![id("FilterFlowBoth"), id("FilterFlowCamera")],
        ),
        (
            "two device models are AND criteria",
            vec![motion, camera],
            vec![id("FilterFlowBoth")],
        ),
        (
            "a device model nothing references",
            vec!["urn:tdm:aws/examples:deviceModel:Thermostat"],
            vec![],
        ),
        (
            "a value that names another kind of entity",
            vec![&a_workflow_id],
            vec![],
        ),
    ];

    for (case, values, mut expected) in cases {
        let mut request = json!({});
        if !values.is_empty() {
            request["filters"] = json!([{"name": "DEVICE_MODEL_ID", "value": values}]);
        }
        let (status, response) = invoke(&router, "SearchFlowTemplates", request).await;
        assert_eq!(status, 200, "case {case:?}: SearchFlowTemplates: {response}");
        expected.sort();
        assert_eq!(
            summary_ids(case, &response),
            expected,
            "case {case:?}: the filter must narrow the listing: {response}"
        );
    }

    // The filter reads the CURRENT definition: a revision that drops the reference drops
    // the workflow from the filtered listing.
    let (status, updated) = invoke(
        &router,
        "UpdateFlowTemplate",
        json!({
            "id": id("FilterFlowMotion"),
            "definition": definition("query FilterFlowMotion { steps }"),
        }),
    )
    .await;
    assert_eq!(status, 200, "UpdateFlowTemplate: {updated}");
    let (_, response) = invoke(
        &router,
        "SearchFlowTemplates",
        json!({"filters": [{"name": "DEVICE_MODEL_ID", "value": [motion]}]}),
    )
    .await;
    assert_eq!(
        summary_ids("after the reference was dropped", &response),
        vec![id("FilterFlowBoth")],
        "the filter must read the latest revision's references: {response}"
    );

    // A filter that names no value states no criterion at all.
    let (status, empty) = invoke(
        &router,
        "SearchFlowTemplates",
        json!({"filters": [{"name": "DEVICE_MODEL_ID", "value": []}]}),
    )
    .await;
    assert_eq!(status, 400, "a filter with no values: {empty}");
    assert_eq!(
        error_code(&empty),
        "InvalidRequestException",
        "unexpected error code in {empty}"
    );

    // RULE: the criteria one search may name are bounded, because every stored workflow
    // is matched against each of them. The largest accepted count, then one past it.
    let a_value = |index: usize| format!("urn:tdm:aws/examples:deviceModel:Model{index}");
    let filter_of = |values: usize| {
        json!({"name": "DEVICE_MODEL_ID",
               "value": (0..values).map(a_value).collect::<Vec<String>>()})
    };
    let bound_cases: Vec<(&str, usize, Value, u16)> = vec![
        (
            "the most values one filter may name",
            MAX_SEARCH_FILTER_VALUES,
            json!([filter_of(MAX_SEARCH_FILTER_VALUES)]),
            200,
        ),
        (
            "one value past that bound",
            MAX_SEARCH_FILTER_VALUES + 1,
            json!([filter_of(MAX_SEARCH_FILTER_VALUES + 1)]),
            400,
        ),
        (
            "the most filters one search may name",
            MAX_SEARCH_FILTERS,
            json!((0..MAX_SEARCH_FILTERS).map(|_| filter_of(1)).collect::<Vec<Value>>()),
            200,
        ),
        (
            "one filter past that bound",
            MAX_SEARCH_FILTERS + 1,
            json!((0..MAX_SEARCH_FILTERS + 1).map(|_| filter_of(1)).collect::<Vec<Value>>()),
            400,
        ),
    ];
    for (case, count, filters, expected_status) in bound_cases {
        let (status, response) =
            invoke(&router, "SearchFlowTemplates", json!({"filters": filters})).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count}): expected HTTP {expected_status}, got {status}: {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {response}"
            );
        }
    }
}
