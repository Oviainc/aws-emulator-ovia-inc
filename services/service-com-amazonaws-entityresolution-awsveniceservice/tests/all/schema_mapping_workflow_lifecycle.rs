// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution schema-mapping + matching-workflow
//! lifecycle.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: each operation of the schema-mapping / matching-workflow loop
//! computes its answer from PERSISTED state. A created record reads back
//! field-for-field, `hasWorkflows` is derived from the workflows that actually
//! reference the schema, a list operation pages over exactly what exists in a stable
//! order, a name is unique so a second create conflicts, and a deleted record is gone
//! from both the reader and the list. Each table spans the class the rule generalises
//! over — attribute type, field count, resolution type, which optional members a
//! request carries, page size — rather than one recorded request.
//!
//! The wire harness here (`Emulator`, `Reply`, `string_member`, the ARN helpers) is
//! `pub(crate)` so the sibling suite `response_messages` drives the SAME path instead
//! of cloning a second harness.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_entityresolution_awsveniceservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `entityresolution`
/// signing name and `region`: the scope is how the router picks the claiming service,
/// and how the service picks the region its ARNs are scoped to. The signature itself
/// is never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
pub(crate) fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDTESTER/20260101/{region}/entityresolution/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// The account id every emulated ARN carries, and the region the harness signs for.
/// The account is private: [`account_id`] is the one accessor every other suite uses.
const ACCOUNT_ID: &str = "000000000000";
pub(crate) const REGION: &str = "us-east-1";

/// A Glue table ARN, which is what `InputSource$inputSourceARN` models for a table
/// input (`^arn:(aws|aws-us-gov|aws-cn):glue:…:(table/<db>/<table>)$`, from
/// `smithy_query(query="shape",shape="InputSource")`). Note it is NOT a schema-mapping
/// ARN: the workflow's reference to a schema mapping is `InputSource$schemaName`.
pub(crate) fn glue_table_arn(table: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/crm/{table}")
}

/// The IAM role ARN a workflow is created with. `roleArn` is modelled as a plain
/// required string on `CreateMatchingWorkflow`, so no pattern constrains it.
pub(crate) fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/entity-resolution")
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: Option<String>,
    pub(crate) body: Value,
}

/// The emulator under test: ONE shared engine with the Entity Resolution service
/// mounted on it, reached the way the production router reaches it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the mounted service holds, kept so a suite can assert about the
    /// STORE itself — what a response cannot show, such as a collection staying bounded
    /// however many requests have run (see [`Emulator::engine`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The same emulator with the account's IAM service mounted BESIDE Entity Resolution
    /// on the one shared engine.
    ///
    /// A write that hands this service a `roleArn` resolves that role by asking IAM over
    /// the emulator's own in-process callback, so IAM has to be mounted for it to answer
    /// at all — and every other suite here, which mounts Entity Resolution alone, is why
    /// a role nothing can be shown about is never refused. The sibling
    /// [`crate::role_resolution`] suite is the one that mounts both.
    pub(crate) fn with_iam() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The shared engine behind this emulator, for the one thing a response cannot
    /// show: how much the store holds.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        self.call_in_region(REGION, method, path, body).await
    }

    /// The same call carrying extra request HEADERS, which is the only way to drive an
    /// operation whose input members are `@httpHeader`-bound: the value is sent
    /// verbatim, so a suite can cover the wire forms a list-valued header takes.
    pub(crate) async fn call_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, String)],
    ) -> Reply {
        self.dispatch(REGION, method, path, body, headers).await
    }

    /// The same call, signed for `region` and addressed to that region's endpoint:
    /// what a client configured for another Region sends. A region-scoped ARN is read
    /// from this scope, so this is how a suite covers more than one of them.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        self.dispatch(region, method, path, body, &[]).await
    }

    /// The one place a request is built and dispatched, so every call above — with or
    /// without extra headers, in any region — travels the same path.
    async fn dispatch(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, String)],
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://entityresolution.{region}.amazonaws.com{path}"
            ))
            .header("authorization", signed_for(region))
            .header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    pub(crate) async fn create_schema_mapping(&self, request: &Value) -> Reply {
        self.call("POST", "/schemas", Some(request)).await
    }

    pub(crate) async fn get_schema_mapping(&self, schema_name: &str) -> Reply {
        self.call("GET", &format!("/schemas/{schema_name}"), None)
            .await
    }

    /// Create a minimal schema mapping under `name` and assert it was stored.
    ///
    /// For a suite whose subject is something else: a matching-workflow write resolves
    /// every `inputSourceConfig[].schemaName` against the stored schema mappings, so a
    /// workflow seeded with a reference needs the mapping it names to exist.
    pub(crate) async fn seed_schema_mapping(&self, name: &str) {
        let request = json!({
            "schemaName": name,
            "mappedInputFields": [
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "email", "type": "EMAIL_ADDRESS" },
            ],
        });
        let reply = self.create_schema_mapping(&request).await;
        assert_eq!(
            reply.status, 200,
            "seeding the schema mapping {name:?} failed, got {reply:?}"
        );
    }

    pub(crate) async fn delete_schema_mapping(&self, schema_name: &str) -> Reply {
        self.call("DELETE", &format!("/schemas/{schema_name}"), None)
            .await
    }

    async fn list_schema_mappings(&self, query: &str) -> Reply {
        self.call("GET", &format!("/schemas{query}"), None).await
    }

    pub(crate) async fn create_matching_workflow(&self, request: &Value) -> Reply {
        self.call("POST", "/matchingworkflows", Some(request)).await
    }

    /// `CreateIdNamespace` — `POST /idnamespaces`
    /// (`smithy_query(query="operation",operation="CreateIdNamespace")`).
    pub(crate) async fn create_id_namespace(&self, request: &Value) -> Reply {
        self.call("POST", "/idnamespaces", Some(request)).await
    }

    /// `GetIdNamespace` — `GET /idnamespaces/{idNamespaceName}`, whose label is an
    /// `EntityNameOrIdNamespaceArn`, so `id_namespace` is either a name or an ARN.
    pub(crate) async fn get_id_namespace(&self, id_namespace: &str) -> Reply {
        self.call("GET", &format!("/idnamespaces/{id_namespace}"), None)
            .await
    }

    /// `UpdateIdNamespace` — `PUT /idnamespaces/{idNamespaceName}`, whose label is an
    /// `EntityName` (a name, never an ARN), with the new configuration as the body.
    pub(crate) async fn update_id_namespace(
        &self,
        id_namespace: &str,
        request: &Value,
    ) -> Reply {
        self.call(
            "PUT",
            &format!("/idnamespaces/{id_namespace}"),
            Some(request),
        )
        .await
    }

    /// `DeleteIdNamespace` — `DELETE /idnamespaces/{idNamespaceName}`, whose label is
    /// an `EntityName` (a name, never an ARN).
    pub(crate) async fn delete_id_namespace(&self, id_namespace: &str) -> Reply {
        self.call("DELETE", &format!("/idnamespaces/{id_namespace}"), None)
            .await
    }

    pub(crate) async fn get_matching_workflow(&self, workflow_name: &str) -> Reply {
        self.call("GET", &format!("/matchingworkflows/{workflow_name}"), None)
            .await
    }

    /// `UpdateMatchingWorkflow` — `PUT /matchingworkflows/{workflowName}`, with the
    /// name as the `@httpLabel` and the new configuration as the body.
    pub(crate) async fn update_matching_workflow(
        &self,
        workflow_name: &str,
        request: &Value,
    ) -> Reply {
        self.call(
            "PUT",
            &format!("/matchingworkflows/{workflow_name}"),
            Some(request),
        )
        .await
    }

    pub(crate) async fn delete_matching_workflow(&self, workflow_name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/matchingworkflows/{workflow_name}"),
            None,
        )
        .await
    }

    async fn list_matching_workflows(&self, query: &str) -> Reply {
        self.call("GET", &format!("/matchingworkflows{query}"), None)
            .await
    }
}

pub(crate) fn string_member(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member `{member}` is not a string in {record:?}"))
        .to_string()
}

/// Assert `reply` is the modelled client failure `code` with HTTP `status`.
///
/// The ONE error-shape assertion in this crate's suites: `response_messages` adds its
/// `message` check on top of this, and the ID-namespace suite uses it as it stands.
pub(crate) fn assert_error_shape(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some(code),
        "case {case:?}: expected {code}, got {reply:?}"
    );
}

pub(crate) fn bool_member(record: &Value, member: &str) -> bool {
    record
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("member `{member}` is not a boolean in {record:?}"))
}

pub(crate) fn list_member<'a>(record: &'a Value, member: &str) -> &'a Vec<Value> {
    record
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("member `{member}` is not a list in {record:?}"))
}

/// A timestamp member, as epoch seconds.
pub(crate) fn timestamp_member(record: &Value, member: &str) -> f64 {
    record
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("member `{member}` is not a timestamp in {record:?}"))
}

/// The ARN Entity Resolution generates for `kind`/`name`, per the modelled
/// `SchemaMappingArn` / `MatchingWorkflowArn` / `IdMappingWorkflowArn` /
/// `IdNamespaceArn` patterns (`arn:aws:entityresolution:<region>:<account>:
/// (schemamapping|matchingworkflow|idmappingworkflow|idnamespace)/<name>`).
pub(crate) fn expected_arn(kind: &str, name: &str) -> String {
    format!("arn:aws:entityresolution:{REGION}:{ACCOUNT_ID}:{kind}/{name}")
}

/// The account id every emulated ARN carries, exposed so a sibling suite can build the
/// ARNs of OTHER services (a Glue table, a KMS key) without a second constant.
pub(crate) fn account_id() -> &'static str {
    ACCOUNT_ID
}

/// One `CreateSchemaMapping` request plus what the rule says must come back from it
/// and from a later `GetSchemaMapping`.
struct SchemaCase {
    case: &'static str,
    schema_name: String,
    request: Value,
    /// `CreateSchemaMapping` models `description` @required on its output while the
    /// input member is optional, so a create carrying none answers the empty string.
    created_description: String,
    /// `GetSchemaMapping` models `description` optional, so an absent one stays absent.
    read_description: Option<&'static str>,
    mapped_input_fields: Value,
    tags: Option<Value>,
}

/// The `CreateSchemaMapping` table.
///
/// It varies the attribute `type` over entries of the whole modelled
/// `SchemaAttributeType` catalog (`smithy_query(query="shape",shape="SchemaInputAttribute")`
/// → `ADDRESS … UNIQUE_ID`), including `IPV6` and `ADDRESS_POSTALCODE`, which the issue
/// never cited; the field count over the modelled `length 2..60` minimum, a middle
/// value and the maximum; `schemaName` over the forms the modelled
/// `^[a-zA-Z_0-9-]*$` admits (letters, digits, `_`, `-`) and over the `length` maximum
/// of 255; and every optional member of both the request and `SchemaInputAttribute`
/// over absent and present.
fn schema_cases() -> Vec<SchemaCase> {
    // The modelled 60-field maximum, built from the catalog so the list is a real
    // 60-element `SchemaInputAttributes` rather than one repeated row. A schema mapping
    // declares EXACTLY ONE `UNIQUE_ID` attribute, so that one is the first field and the
    // other 59 cycle the rest of the catalog.
    let other_types: Vec<&str> = SCHEMA_ATTRIBUTE_TYPES
        .iter()
        .copied()
        .filter(|attribute_type| *attribute_type != "UNIQUE_ID")
        .collect();
    let widest: Vec<Value> = (0..60)
        .map(|index| match index {
            0 => json!({ "fieldName": "id", "type": "UNIQUE_ID" }),
            _ => json!({
                "fieldName": format!("col_{index}"),
                "type": other_types[(index - 1) % other_types.len()],
            }),
        })
        .collect();

    vec![
        SchemaCase {
            case: "the modelled minimum of two fields, no optional members",
            schema_name: "customers".to_string(),
            request: json!({
                "schemaName": "customers",
                "mappedInputFields": [
                    { "fieldName": "id", "type": "UNIQUE_ID" },
                    { "fieldName": "name", "type": "NAME" },
                ],
            }),
            created_description: String::new(),
            read_description: None,
            mapped_input_fields: json!([
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "name", "type": "NAME" },
            ]),
            tags: None,
        },
        SchemaCase {
            case: "grouped name parts with matchKey, description and tags",
            schema_name: "crm_people-1".to_string(),
            request: json!({
                "schemaName": "crm_people-1",
                "description": "people exported from the CRM",
                "mappedInputFields": [
                    { "fieldName": "id", "type": "UNIQUE_ID" },
                    { "fieldName": "first", "type": "NAME_FIRST", "groupName": "NAME",
                      "matchKey": "fullname" },
                    { "fieldName": "last", "type": "NAME_LAST", "groupName": "NAME",
                      "matchKey": "fullname" },
                ],
                "tags": { "team": "identity", "env": "test" },
            }),
            created_description: "people exported from the CRM".to_string(),
            read_description: Some("people exported from the CRM"),
            mapped_input_fields: json!([
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "first", "type": "NAME_FIRST", "groupName": "NAME",
                  "matchKey": "fullname" },
                { "fieldName": "last", "type": "NAME_LAST", "groupName": "NAME",
                  "matchKey": "fullname" },
            ]),
            tags: Some(json!({ "team": "identity", "env": "test" })),
        },
        SchemaCase {
            case: "hashed and sub-typed network attributes the issue never cited",
            schema_name: "DEVICES0".to_string(),
            request: json!({
                "schemaName": "DEVICES0",
                "description": "",
                "mappedInputFields": [
                    { "fieldName": "device id", "type": "UNIQUE_ID" },
                    { "fieldName": "v6", "type": "IPV6", "hashed": true },
                    { "fieldName": "zip", "type": "ADDRESS_POSTALCODE",
                      "groupName": "ADDRESS", "subType": "home", "hashed": false },
                    { "fieldName": "ad id", "type": "MAID" },
                ],
                "tags": {},
            }),
            created_description: String::new(),
            read_description: Some(""),
            mapped_input_fields: json!([
                { "fieldName": "device id", "type": "UNIQUE_ID" },
                { "fieldName": "v6", "type": "IPV6", "hashed": true },
                { "fieldName": "zip", "type": "ADDRESS_POSTALCODE",
                  "groupName": "ADDRESS", "subType": "home", "hashed": false },
                { "fieldName": "ad id", "type": "MAID" },
            ]),
            tags: Some(json!({})),
        },
        SchemaCase {
            case: "schemaName at the modelled length maximum of 255",
            schema_name: "s".repeat(255),
            request: json!({
                "schemaName": "s".repeat(255),
                "mappedInputFields": [
                    { "fieldName": "id", "type": "UNIQUE_ID" },
                    { "fieldName": "phone", "type": "PHONE_NUMBER" },
                ],
            }),
            created_description: String::new(),
            read_description: None,
            mapped_input_fields: json!([
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "phone", "type": "PHONE_NUMBER" },
            ]),
            tags: None,
        },
        SchemaCase {
            case: "the modelled maximum of 60 mapped input fields",
            schema_name: "widest".to_string(),
            request: json!({
                "schemaName": "widest",
                "mappedInputFields": widest.clone(),
            }),
            created_description: String::new(),
            read_description: None,
            mapped_input_fields: Value::Array(widest),
            tags: None,
        },
    ]
}

/// The values `SchemaAttributeType` accepts on the wire, from
/// `smithy_query(query="shape",shape="SchemaInputAttribute")`.
const SCHEMA_ATTRIBUTE_TYPES: &[&str] = &[
    "ADDRESS",
    "ADDRESS_CITY",
    "ADDRESS_COUNTRY",
    "ADDRESS_POSTALCODE",
    "ADDRESS_STATE",
    "ADDRESS_STREET1",
    "ADDRESS_STREET2",
    "ADDRESS_STREET3",
    "DATE",
    "EMAIL_ADDRESS",
    "IPV4",
    "IPV6",
    "MAID",
    "NAME",
    "NAME_FIRST",
    "NAME_LAST",
    "NAME_MIDDLE",
    "PHONE",
    "PHONE_COUNTRYCODE",
    "PHONE_NUMBER",
    "PROVIDER_ID",
    "STRING",
    "UNIQUE_ID",
];

#[tokio::test]
async fn create_schema_mapping_persists_a_record_get_schema_mapping_reads_it_back() {
    let emulator = Emulator::new();

    for case in schema_cases() {
        let label = case.case;
        let name = &case.schema_name;

        let created = emulator.create_schema_mapping(&case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateSchemaMapping, got {created:?}"
        );
        assert_eq!(
            string_member(&created.body, "schemaName"),
            *name,
            "case {label:?}: CreateSchemaMapping must answer the stored schemaName, got {:?}",
            created.body
        );
        assert_eq!(
            string_member(&created.body, "schemaArn"),
            expected_arn("schemamapping", name),
            "case {label:?}: expected the generated schemamapping ARN, got {:?}",
            created.body
        );
        assert_eq!(
            string_member(&created.body, "description"),
            case.created_description,
            "case {label:?}: unexpected description on the create response: {:?}",
            created.body
        );
        assert_eq!(
            created.body.get("mappedInputFields"),
            Some(&case.mapped_input_fields),
            "case {label:?}: CreateSchemaMapping must echo the STORED mappedInputFields, got {:?}",
            created.body
        );

        // The round trip: the reader clones what the writer stored.
        let read = emulator.get_schema_mapping(name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetSchemaMapping, got {read:?}"
        );
        assert_eq!(
            string_member(&read.body, "schemaName"),
            *name,
            "case {label:?}: GetSchemaMapping returned another record: {:?}",
            read.body
        );
        assert_eq!(
            string_member(&read.body, "schemaArn"),
            expected_arn("schemamapping", name),
            "case {label:?}: the stored ARN must survive the round trip, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("description").and_then(Value::as_str),
            case.read_description,
            "case {label:?}: unexpected description on the read: {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("mappedInputFields"),
            Some(&case.mapped_input_fields),
            "case {label:?}: GetSchemaMapping must return the stored mappedInputFields, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("tags"),
            case.tags.as_ref(),
            "case {label:?}: GetSchemaMapping must return the stored tags, got {:?}",
            read.body
        );
        assert!(
            !bool_member(&read.body, "hasWorkflows"),
            "case {label:?}: a schema mapping no workflow references must report \
             hasWorkflows=false, got {:?}",
            read.body
        );

        // A freshly created record has not been updated since, and both stamps are
        // stable across reads (nothing is re-derived per call).
        let created_at = timestamp_member(&read.body, "createdAt");
        let updated_at = timestamp_member(&read.body, "updatedAt");
        assert!(
            created_at > 0.0,
            "case {label:?}: expected a real createdAt, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {label:?}: a never-updated record must have createdAt == updatedAt, \
             got {created_at} and {updated_at}"
        );
        let again = emulator.get_schema_mapping(name).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a read must not change what the next reader sees"
        );
    }
}

/// One `CreateMatchingWorkflow` request plus what the rule says must come back from it
/// and from a later `GetMatchingWorkflow`.
struct WorkflowCase {
    case: &'static str,
    workflow_name: &'static str,
    request: Value,
    /// The `schemaName`s this workflow's `inputSourceConfig` references.
    references: Vec<&'static str>,
    resolution_type: &'static str,
    tags: Option<Value>,
}

/// The `CreateMatchingWorkflow` table.
///
/// It varies `resolutionTechniques.resolutionType` over the WHOLE modelled
/// `ResolutionType` enum (`ML_MATCHING | PROVIDER | RULE_MATCHING`, from
/// `smithy_query(query="shape",shape="ResolutionTechniques")`), the
/// `inputSourceConfig` length over the modelled `length 1..20` minimum and a middle
/// value, and each optional member — `description`, `incrementalRunConfig`, `tags`,
/// `applyNormalization`, `matchPurpose`, `enableRealTimeMatching` — over absent and
/// present.
fn workflow_cases() -> Vec<WorkflowCase> {
    vec![
        WorkflowCase {
            case: "rule matching over one input source, no optional members",
            workflow_name: "wf-rules",
            request: json!({
                "workflowName": "wf-rules",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers" },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/rules",
                    "output": [{ "name": "id" }, { "name": "name", "hashed": true }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "ruleBasedProperties": {
                        "rules": [{ "ruleName": "by name", "matchingKeys": ["name"] }],
                        "attributeMatchingModel": "ONE_TO_ONE",
                    },
                },
                "roleArn": role_arn(),
            }),
            references: vec!["customers"],
            resolution_type: "RULE_MATCHING",
            tags: None,
        },
        WorkflowCase {
            case: "ML matching over three input sources with description, tags and an \
                   incremental run config",
            workflow_name: "wf_ml_0",
            request: json!({
                "workflowName": "wf_ml_0",
                "description": "nightly consolidation",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers",
                      "applyNormalization": true },
                    { "inputSourceARN": glue_table_arn("orders"), "schemaName": "crm_people-1" },
                    { "inputSourceARN": glue_table_arn("devices"), "schemaName": "DEVICES0",
                      "applyNormalization": false },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/ml",
                    "output": [{ "name": "id" }, { "name": "email", "hashed": false }],
                    "applyNormalization": true,
                }],
                "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
                "incrementalRunConfig": { "incrementalRunType": "IMMEDIATE" },
                "roleArn": role_arn(),
                "tags": { "owner": "identity" },
            }),
            references: vec!["customers", "crm_people-1", "DEVICES0"],
            resolution_type: "ML_MATCHING",
            tags: Some(json!({ "owner": "identity" })),
        },
        WorkflowCase {
            case: "provider matching with provider properties and real-time matching off",
            workflow_name: "WF2",
            request: json!({
                "workflowName": "WF2",
                "description": "",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("devices"), "schemaName": "DEVICES0" },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/provider",
                    "output": [{ "name": "device id" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "PROVIDER",
                    "enableRealTimeMatching": false,
                    "providerProperties": {
                        // `ProviderServiceArn` is modelled with an EMPTY account
                        // segment: `…:entityresolution:<region>::providerservice/<p>/<s>`.
                        "providerServiceArn": format!(
                            "arn:aws:entityresolution:{REGION}::providerservice/testprovider/assignment"
                        ),
                        "intermediateSourceConfiguration": {
                            "intermediateS3Path": "s3://er-intermediate/provider",
                        },
                    },
                },
                "roleArn": role_arn(),
                "tags": {},
            }),
            references: vec!["DEVICES0"],
            resolution_type: "PROVIDER",
            tags: Some(json!({})),
        },
        WorkflowCase {
            case: "rule matching with several rules, a match purpose and many-to-many",
            workflow_name: "wf3",
            request: json!({
                "workflowName": "wf3",
                "inputSourceConfig": [
                    { "inputSourceARN": glue_table_arn("customers"), "schemaName": "customers" },
                    { "inputSourceARN": glue_table_arn("people"), "schemaName": "crm_people-1" },
                ],
                "outputSourceConfig": [{
                    "outputS3Path": "s3://er-output/rules3",
                    "output": [{ "name": "id" }, { "name": "first" }, { "name": "last" }],
                }],
                "resolutionTechniques": {
                    "resolutionType": "RULE_MATCHING",
                    "enableRealTimeMatching": true,
                    "ruleBasedProperties": {
                        "rules": [
                            { "ruleName": "by fullname", "matchingKeys": ["fullname"] },
                            { "ruleName": "by id", "matchingKeys": ["id"] },
                        ],
                        "attributeMatchingModel": "MANY_TO_MANY",
                        "matchPurpose": "IDENTIFIER_GENERATION",
                    },
                },
                "roleArn": role_arn(),
            }),
            references: vec!["customers", "crm_people-1"],
            resolution_type: "RULE_MATCHING",
            tags: None,
        },
    ]
}

/// The members `CreateMatchingWorkflow` and `GetMatchingWorkflow` both model, which
/// must therefore be identical: both are served from the one stored record.
const SHARED_WORKFLOW_MEMBERS: &[&str] = &[
    "workflowName",
    "workflowArn",
    "description",
    "inputSourceConfig",
    "outputSourceConfig",
    "resolutionTechniques",
    "incrementalRunConfig",
    "roleArn",
];

/// Create the schema mappings `workflow_cases` references, so the workflow table runs
/// against a store that holds the schemas its input sources name.
async fn seed_schema_mappings(emulator: &Emulator) {
    for case in schema_cases() {
        let reply = emulator.create_schema_mapping(&case.request).await;
        assert_eq!(
            reply.status, 200,
            "seeding schema mapping for case {:?} failed: {reply:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn create_matching_workflow_persists_a_record_get_matching_workflow_reads_it_back() {
    let emulator = Emulator::new();
    seed_schema_mappings(&emulator).await;

    for case in workflow_cases() {
        let label = case.case;
        let name = case.workflow_name;

        let created = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );
        assert_eq!(
            string_member(&created.body, "workflowArn"),
            expected_arn("matchingworkflow", name),
            "case {label:?}: expected the generated matchingworkflow ARN, got {:?}",
            created.body
        );

        let read = emulator.get_matching_workflow(name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetMatchingWorkflow, got {read:?}"
        );

        // Every member the two operations share is served from the SAME stored
        // record, so create and read cannot disagree on any of them.
        for member in SHARED_WORKFLOW_MEMBERS {
            assert_eq!(
                read.body.get(member),
                created.body.get(member),
                "case {label:?}: member `{member}` differs between the create response \
                 and the read: {:?} vs {:?}",
                created.body.get(member),
                read.body.get(member)
            );
        }

        // And the configuration the request carried is what was stored.
        assert_eq!(
            read.body.get("inputSourceConfig"),
            case.request.get("inputSourceConfig"),
            "case {label:?}: inputSourceConfig must read back as created, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("resolutionTechniques"),
            case.request.get("resolutionTechniques"),
            "case {label:?}: resolutionTechniques must read back as created, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("incrementalRunConfig"),
            case.request.get("incrementalRunConfig"),
            "case {label:?}: incrementalRunConfig must read back as created, got {:?}",
            read.body
        );
        assert_eq!(
            string_member(&read.body, "roleArn"),
            role_arn(),
            "case {label:?}: roleArn must read back as created, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("tags"),
            case.tags.as_ref(),
            "case {label:?}: GetMatchingWorkflow must return the stored tags, got {:?}",
            read.body
        );
        assert_eq!(
            list_member(&read.body, "outputSourceConfig").len(),
            1,
            "case {label:?}: outputSourceConfig is modelled length 1..1, got {:?}",
            read.body
        );

        let created_at = timestamp_member(&read.body, "createdAt");
        let updated_at = timestamp_member(&read.body, "updatedAt");
        assert!(
            created_at > 0.0,
            "case {label:?}: expected a real createdAt, got {created_at}"
        );
        assert_eq!(
            created_at, updated_at,
            "case {label:?}: a never-updated workflow must have createdAt == updatedAt, \
             got {created_at} and {updated_at}"
        );

        // The workflow's own summary reports the stored resolutionType, not a constant.
        let listed = emulator.list_matching_workflows("").await;
        let summary = list_member(&listed.body, "workflowSummaries")
            .iter()
            .find(|entry| string_member(entry, "workflowName") == name)
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: {name} missing from the list"));
        assert_eq!(
            string_member(&summary, "resolutionType"),
            case.resolution_type,
            "case {label:?}: the summary must carry the stored resolutionType, got {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "workflowArn"),
            expected_arn("matchingworkflow", name),
            "case {label:?}: the summary must carry the stored ARN, got {summary:?}"
        );
        assert_eq!(
            timestamp_member(&summary, "createdAt"),
            created_at,
            "case {label:?}: the summary's createdAt must be the stored one, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_duplicate_name_is_refused_with_conflict_and_leaves_the_stored_record_alone() {
    let emulator = Emulator::new();
    seed_schema_mappings(&emulator).await;

    // Every create in both tables is a case: re-issuing it must conflict, whatever
    // the record's shape, and must not replace what is stored.
    for case in schema_cases() {
        let label = case.case;
        let before = emulator.get_schema_mapping(&case.schema_name).await;
        let duplicate = emulator.create_schema_mapping(&case.request).await;
        assert_eq!(
            duplicate.status, 400,
            "case {label:?}: a duplicate schemaName must be refused, got {duplicate:?}"
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?}",
            duplicate.error_code
        );
        let after = emulator.get_schema_mapping(&case.schema_name).await;
        assert_eq!(
            after.body, before.body,
            "case {label:?}: a refused create must leave the stored record untouched"
        );
    }

    for case in workflow_cases() {
        let label = case.case;
        let first = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from the first create, got {first:?}"
        );
        let before = emulator.get_matching_workflow(case.workflow_name).await;
        let duplicate = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            duplicate.status, 400,
            "case {label:?}: a duplicate workflowName must be refused, got {duplicate:?}"
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?}",
            duplicate.error_code
        );
        let after = emulator.get_matching_workflow(case.workflow_name).await;
        assert_eq!(
            after.body, before.body,
            "case {label:?}: a refused create must leave the stored record untouched"
        );
    }
}

#[tokio::test]
async fn has_workflows_is_derived_from_the_workflows_that_reference_the_schema() {
    let emulator = Emulator::new();
    seed_schema_mappings(&emulator).await;

    // With no workflows at all, no schema mapping is referenced.
    for case in schema_cases() {
        let read = emulator.get_schema_mapping(&case.schema_name).await;
        assert!(
            !bool_member(&read.body, "hasWorkflows"),
            "case {:?}: with no workflows stored, hasWorkflows must be false, got {:?}",
            case.case,
            read.body
        );
    }

    // Each workflow of the table flips exactly the schema mappings its own
    // `inputSourceConfig` names — and nothing else.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for case in workflow_cases() {
        let label = case.case;
        let created = emulator.create_matching_workflow(&case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateMatchingWorkflow, got {created:?}"
        );
        referenced.extend(case.references.iter().map(|name| (*name).to_string()));

        for schema in schema_cases() {
            let expected = referenced.contains(&schema.schema_name);
            let read = emulator.get_schema_mapping(&schema.schema_name).await;
            assert_eq!(
                bool_member(&read.body, "hasWorkflows"),
                expected,
                "case {label:?}: hasWorkflows for schema {:?} must be {expected}, got {:?}",
                schema.schema_name,
                read.body
            );

            // ListSchemaMappings derives it the same way GetSchemaMapping does.
            let listed = emulator.list_schema_mappings("?maxResults=25").await;
            let summary = list_member(&listed.body, "schemaList")
                .iter()
                .find(|entry| string_member(entry, "schemaName") == schema.schema_name)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: {:?} missing from ListSchemaMappings",
                        schema.schema_name
                    )
                });
            assert_eq!(
                bool_member(&summary, "hasWorkflows"),
                expected,
                "case {label:?}: the summary's hasWorkflows for {:?} must be {expected}, \
                 got {summary:?}",
                schema.schema_name
            );
        }
    }

    // Deleting the workflows releases the schema mappings again: the flag is derived,
    // never a stored leftover.
    for case in workflow_cases() {
        let deleted = emulator.delete_matching_workflow(case.workflow_name).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected HTTP 200 from DeleteMatchingWorkflow, got {deleted:?}",
            case.case
        );
    }
    for case in schema_cases() {
        let read = emulator.get_schema_mapping(&case.schema_name).await;
        assert!(
            !bool_member(&read.body, "hasWorkflows"),
            "case {:?}: once every workflow is gone, hasWorkflows must be false again, got {:?}",
            case.case,
            read.body
        );
    }
}

#[tokio::test]
async fn deleting_a_referenced_schema_mapping_conflicts_until_the_workflow_is_gone() {
    let emulator = Emulator::new();
    seed_schema_mappings(&emulator).await;
    for case in workflow_cases() {
        assert_eq!(
            emulator.create_matching_workflow(&case.request).await.status,
            200,
            "case {:?}: seeding the workflow failed",
            case.case
        );
    }

    // Every schema mapping named by some workflow's inputSourceConfig is refused.
    let referenced: BTreeSet<String> = workflow_cases()
        .iter()
        .flat_map(|case| case.references.iter().map(|name| (*name).to_string()))
        .collect();
    for name in &referenced {
        let refused = emulator.delete_schema_mapping(name).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: deleting a referenced schema mapping must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {name:?}: expected ConflictException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            emulator.get_schema_mapping(name).await.status,
            200,
            "case {name:?}: a refused delete must leave the record in the store"
        );
    }

    // An unreferenced one deletes straight away, and is then gone from the reader and
    // from the list.
    for case in schema_cases() {
        if referenced.contains(&case.schema_name) {
            continue;
        }
        let name = &case.schema_name;
        let deleted = emulator.delete_schema_mapping(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected HTTP 200 from DeleteSchemaMapping, got {deleted:?}"
        );
        assert!(
            !string_member(&deleted.body, "message").is_empty(),
            "case {name:?}: DeleteSchemaMapping models a required message, got {:?}",
            deleted.body
        );
        assert_eq!(
            emulator.get_schema_mapping(name).await.status,
            404,
            "case {name:?}: a deleted schema mapping must no longer be readable"
        );
        assert!(
            !listed_schema_names(&emulator).await.contains(name),
            "case {name:?}: a deleted schema mapping must be gone from ListSchemaMappings"
        );
    }

    // Once the workflows are deleted, the previously-referenced schemas delete too.
    for case in workflow_cases() {
        let name = case.workflow_name;
        let deleted = emulator.delete_matching_workflow(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected HTTP 200 from DeleteMatchingWorkflow, got {deleted:?}"
        );
        assert_eq!(
            emulator.get_matching_workflow(name).await.status,
            404,
            "case {name:?}: a deleted workflow must no longer be readable"
        );
        assert!(
            !listed_workflow_names(&emulator).await.contains(name),
            "case {name:?}: a deleted workflow must be gone from ListMatchingWorkflows"
        );
    }
    for name in &referenced {
        let deleted = emulator.delete_schema_mapping(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: with no workflow referencing it, the delete must succeed, \
             got {deleted:?}"
        );
    }
}

/// Every `schemaName` the store holds, walked through `ListSchemaMappings`.
async fn listed_schema_names(emulator: &Emulator) -> BTreeSet<String> {
    emulator
        .list_schema_mappings("")
        .await
        .body
        .get("schemaList")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| string_member(entry, "schemaName"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `workflowName` the store holds, walked through `ListMatchingWorkflows`.
async fn listed_workflow_names(emulator: &Emulator) -> BTreeSet<String> {
    emulator
        .list_matching_workflows("")
        .await
        .body
        .get("workflowSummaries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| string_member(entry, "workflowName"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn both_lists_page_at_max_results_and_resume_exactly_where_the_token_points() {
    let emulator = Emulator::new();

    // Six records per collection, named so the stable (name) order is NOT the
    // creation order — a page boundary that happened to follow insertion order would
    // not survive this.
    let names = ["e5", "a1", "d4", "b2", "f6", "c3"];
    for name in names {
        let schema = json!({
            "schemaName": name,
            "mappedInputFields": [
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "name", "type": "NAME" },
            ],
        });
        assert_eq!(
            emulator.create_schema_mapping(&schema).await.status,
            200,
            "seeding schema mapping {name} failed"
        );
        let workflow = json!({
            "workflowName": name,
            "inputSourceConfig": [
                { "inputSourceARN": glue_table_arn("customers"), "schemaName": name },
            ],
            "outputSourceConfig": [{
                "outputS3Path": "s3://er-output/page",
                "output": [{ "name": "id" }],
            }],
            "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
            "roleArn": role_arn(),
        });
        assert_eq!(
            emulator.create_matching_workflow(&workflow).await.status,
            200,
            "seeding workflow {name} failed"
        );
    }
    let expected: Vec<String> = {
        let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        sorted.sort();
        sorted
    };

    // The pagination invariant, checked EXHAUSTIVELY over every page size the
    // collection admits — 1..=6 plus "no bound at all" — rather than on sampled
    // sizes: concatenating the pages reproduces the full collection in order,
    // exactly once each; every page but the last is full and carries a token; the
    // last carries none.
    for bound in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)] {
        for (collection, items) in [("schemas", "schemaList"), ("workflows", "workflowSummaries")] {
            let label = format!("{collection} with maxResults={bound:?}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;

            loop {
                let query = match (bound, token.as_deref()) {
                    (None, None) => String::new(),
                    (None, Some(next)) => format!("?nextToken={next}"),
                    (Some(size), None) => format!("?maxResults={size}"),
                    (Some(size), Some(next)) => format!("?maxResults={size}&nextToken={next}"),
                };
                let reply = if collection == "schemas" {
                    emulator.list_schema_mappings(&query).await
                } else {
                    emulator.list_matching_workflows(&query).await
                };
                assert_eq!(
                    reply.status, 200,
                    "case {label:?}: expected HTTP 200, got {reply:?}"
                );
                let page = list_member(&reply.body, items);
                let member = if collection == "schemas" {
                    "schemaName"
                } else {
                    "workflowName"
                };
                let page_names: Vec<String> = page
                    .iter()
                    .map(|entry| string_member(entry, member))
                    .collect();
                token = reply
                    .body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);

                if let (Some(size), Some(_)) = (bound, token.as_deref()) {
                    assert_eq!(
                        page_names.len(),
                        size,
                        "case {label:?}: a page followed by a token must be full, got {page_names:?}"
                    );
                }
                assert!(
                    !page_names.is_empty() || token.is_none(),
                    "case {label:?}: an empty page must not carry a token"
                );
                walked.extend(page_names);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {label:?}: paging did not terminate; walked {walked:?}"
                );
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                walked, expected,
                "case {label:?}: the pages concatenated must be the whole collection in \
                 name order, got {walked:?}"
            );
            let page_size = bound.unwrap_or(expected.len());
            assert_eq!(
                pages,
                expected.len().div_ceil(page_size),
                "case {label:?}: expected {} pages of at most {page_size}, took {pages}",
                expected.len().div_ceil(page_size)
            );
        }
    }

    // A token naming a record that was deleted in between still resumes at the next
    // name rather than failing: that is what makes a page boundary survive a
    // concurrent delete.
    let first_page = emulator.list_matching_workflows("?maxResults=2").await;
    let token = string_member(&first_page.body, "nextToken");
    assert_eq!(
        token, "c3",
        "case \"token is the first key not returned\": got {:?}",
        first_page.body
    );
    assert_eq!(
        emulator.delete_matching_workflow(&token).await.status,
        200,
        "case \"delete the record the token names\": expected HTTP 200"
    );
    let resumed = emulator
        .list_matching_workflows(&format!("?nextToken={token}"))
        .await;
    let resumed_names: Vec<String> = list_member(&resumed.body, "workflowSummaries")
        .iter()
        .map(|entry| string_member(entry, "workflowName"))
        .collect();
    assert_eq!(
        resumed_names,
        vec!["d4".to_string(), "e5".to_string(), "f6".to_string()],
        "case \"resume past a deleted token\": expected the names after the deleted one, \
         got {resumed_names:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();

    // The modelled `NextToken` (`^[a-zA-Z_0-9-=+/]*$`) is WIDER than the resource
    // names this service issues as tokens (`^[a-zA-Z_0-9-]*$`), so these satisfy the
    // member's own constraints and still cannot be a token this service minted.
    for token in ["abc=", "+", "a/b", "ZGVmZ2g=", "x".repeat(256).as_str()] {
        for collection in ["schemas", "workflows"] {
            let query = format!("?nextToken={token}");
            let reply = if collection == "schemas" {
                emulator.list_schema_mappings(&query).await
            } else {
                emulator.list_matching_workflows(&query).await
            };
            assert_eq!(
                reply.status, 400,
                "case {collection}/{token:?}: an unissued nextToken must be refused, got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ValidationException"),
                "case {collection}/{token:?}: expected ValidationException, got {:?}",
                reply.error_code
            );
        }
    }
}

#[tokio::test]
async fn a_create_past_the_store_quota_is_refused_and_a_delete_makes_room_again() {
    use service_com_amazonaws_entityresolution_awsveniceservice::state::{
        MAX_MATCHING_WORKFLOWS, MAX_SCHEMA_MAPPINGS,
    };

    let emulator = Emulator::new();

    // Both stores are bounded, so each is tested at its own cap: fill it, show the
    // next create is refused with the modelled ExceedsLimitException (HTTP 402) and
    // stores nothing, then show a delete releases room.
    let cases: [(&str, usize); 2] = [
        ("schema mappings", MAX_SCHEMA_MAPPINGS),
        ("matching workflows", MAX_MATCHING_WORKFLOWS),
    ];

    for (label, maximum) in cases {
        let schemas = label == "schema mappings";
        for index in 0..maximum {
            let name = format!("q{index}");
            let reply = if schemas {
                emulator.create_schema_mapping(&quota_schema(&name)).await
            } else {
                emulator
                    .create_matching_workflow(&quota_workflow(&name))
                    .await
            };
            assert_eq!(
                reply.status, 200,
                "case {label:?}: create {index} of {maximum} must succeed, got {reply:?}"
            );
        }

        let past = if schemas {
            emulator
                .create_schema_mapping(&quota_schema("overflow"))
                .await
        } else {
            emulator
                .create_matching_workflow(&quota_workflow("overflow"))
                .await
        };
        assert_eq!(
            past.status, 402,
            "case {label:?}: a create past the quota of {maximum} must answer HTTP 402, \
             got {past:?}"
        );
        assert_eq!(
            past.error_code.as_deref(),
            Some("ExceedsLimitException"),
            "case {label:?}: expected ExceedsLimitException, got {:?}",
            past.error_code
        );
        let absent = if schemas {
            emulator.get_schema_mapping("overflow").await
        } else {
            emulator.get_matching_workflow("overflow").await
        };
        assert_eq!(
            absent.status, 404,
            "case {label:?}: a refused create must store no record, got {absent:?}"
        );

        // A delete releases room, so the bound needs no TTL or eviction.
        let released = if schemas {
            emulator.delete_schema_mapping("q0").await
        } else {
            emulator.delete_matching_workflow("q0").await
        };
        assert_eq!(
            released.status, 200,
            "case {label:?}: expected HTTP 200 from the delete, got {released:?}"
        );
        let retried = if schemas {
            emulator
                .create_schema_mapping(&quota_schema("overflow"))
                .await
        } else {
            emulator
                .create_matching_workflow(&quota_workflow("overflow"))
                .await
        };
        assert_eq!(
            retried.status, 200,
            "case {label:?}: with room released the create must succeed, got {retried:?}"
        );
    }
}

/// The smallest `CreateSchemaMapping` request the model admits, for the quota walk.
fn quota_schema(name: &str) -> Value {
    json!({
        "schemaName": name,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "name", "type": "NAME" },
        ],
    })
}

/// The smallest `CreateMatchingWorkflow` request the model admits, for the quota walk.
///
/// Its one input source references `q1`, a schema mapping the SCHEMA half of the walk
/// above created and did not delete: a matching-workflow write resolves every
/// `inputSourceConfig[].schemaName` against the stored mappings, and the schema store is
/// at its own cap by the time the workflow half runs, so the reference names one of the
/// records already there rather than adding another.
fn quota_workflow(name: &str) -> Value {
    json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn("customers"), "schemaName": "q1" },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/quota",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    })
}

#[tokio::test]
async fn an_unknown_name_answers_resource_not_found_on_every_reader_and_delete() {
    let emulator = Emulator::new();
    seed_schema_mappings(&emulator).await;

    // The class: a name that is well-formed per `EntityName` but names nothing in the
    // store — never created, or created and then deleted.
    assert_eq!(
        emulator.delete_schema_mapping("customers").await.status,
        200,
        "the fixture schema mapping must delete cleanly"
    );

    let absent = ["customers", "nothing-here", "X_9", &"z".repeat(255)];
    for name in absent {
        for (operation, reply) in [
            ("GetSchemaMapping", emulator.get_schema_mapping(name).await),
            (
                "DeleteSchemaMapping",
                emulator.delete_schema_mapping(name).await,
            ),
            (
                "GetMatchingWorkflow",
                emulator.get_matching_workflow(name).await,
            ),
            (
                "DeleteMatchingWorkflow",
                emulator.delete_matching_workflow(name).await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {operation}/{name:?}: expected HTTP 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation}/{name:?}: expected ResourceNotFoundException, got {:?}",
                reply.error_code
            );
        }
    }
}
