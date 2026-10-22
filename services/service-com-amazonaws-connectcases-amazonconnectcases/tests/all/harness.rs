// SPDX-License-Identifier: CC0-1.0

//! The shared end-to-end harness for this crate's suites: one emulator behind
//! the real `MultiServiceRouter`, plus the small helpers every suite needs to
//! drive the Cases lifecycle (domain -> field -> template -> case) over the
//! wire the SDK speaks.
//!
//! It lives in its own module so a new suite reuses it instead of growing a
//! second copy of the same request plumbing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::FIELD_TYPE_VALUES;
use service_com_amazonaws_connectcases_amazonconnectcases::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to this service's signing name
/// (`cases`, per the crate's `OPERATIONS.md`), which is what the router routes
/// on and what the handlers read the ARN region from.
pub fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cases/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: a single shared engine behind the production router.
pub struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    pub fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body
            .map(|body| serde_json::to_vec(&body).expect("serialize the request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://cases.{}.amazonaws.com{path}", self.region))
            .header("authorization", authorization(&self.region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, json)
    }

    /// Send a call that must succeed, and hand back its parsed body.
    pub async fn ok(&self, case: &str, method: &str, path: &str, body: Value) -> Value {
        let (status, json) = self.send(method, path, Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {json:?}"
        );
        json
    }

    /// Send a call that must fail, and hand back its status and wire error code.
    pub async fn failure(
        &self,
        case: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String) {
        let (status, json) = self.send(method, path, body).await;
        let code = json
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string();
        assert_ne!(
            status, 200,
            "case {case:?}: expected a failure, got HTTP 200 with body {json:?}"
        );
        (status, code)
    }

    /// Send a call that must fail, and hand back its status, wire error code and message.
    pub async fn failure_detail(
        &self,
        case: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String, String) {
        let (status, json) = self.send(method, path, body).await;
        assert_ne!(
            status, 200,
            "case {case:?}: expected a failure, got HTTP 200 with body {json:?}"
        );
        let code = json
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .to_string();
        let message = json
            .get("message")
            .or_else(|| json.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string();
        (status, code, message)
    }
}

pub fn string_at(json: &Value, member: &str, case: &str) -> String {
    json.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no string member {member:?} in {json:?}"))
        .to_string()
}

/// Every `FieldValueUnion` variant, each paired with the `FieldType` whose
/// values it carries. The model gives a case field's value the field's own
/// declared type (`FieldValueUnion` in `src/ops/types/shared_types.rs`,
/// `FieldType` in `FIELD_TYPE_VALUES`): `booleanValue` is a Boolean, a
/// `doubleValue` a Number, a `userArnValue` a User, and the string-valued types
/// take `stringValue`. `emptyValue` is the absent value and fits any type — the
/// row below writes it against a `SingleSelect` field. Each row is a value whose
/// stored form a read must reproduce exactly, variant included.
pub fn field_value_variants() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "stringValue",
            "Text",
            json!({ "stringValue": "a customer note" }),
        ),
        ("doubleValue", "Number", json!({ "doubleValue": 42.5 })),
        ("booleanValue", "Boolean", json!({ "booleanValue": true })),
        ("emptyValue", "SingleSelect", json!({ "emptyValue": {} })),
        (
            "userArnValue",
            "User",
            json!({ "userArnValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1" }),
        ),
    ]
}

/// The id of the field of `field_type` among `fields`, as
/// `fields_of_every_type` created them.
pub fn field_of_type(fields: &[(&'static str, String)], field_type: &str) -> String {
    fields
        .iter()
        .find(|(created, _)| *created == field_type)
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| panic!("no {field_type} field was created: {fields:?}"))
}

/// Create a domain and return its id.
pub async fn domain(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(name, "POST", "/domains", json!({ "name": name }))
        .await;
    string_at(&created, "domainId", name)
}

/// Create one field per `FieldType` the model declares, returning
/// `(type, fieldId)` in that order.
pub async fn fields_of_every_type(
    emulator: &Emulator,
    domain_id: &str,
) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for field_type in FIELD_TYPE_VALUES {
        let case = format!("CreateField type={field_type}");
        let body = json!({ "name": format!("field-{field_type}"), "type": field_type });
        let response = emulator
            .ok(&case, "POST", &format!("/domains/{domain_id}/fields"), body)
            .await;
        let field_id = string_at(&response, "fieldId", &case);
        assert!(
            is_minted_id(&field_id),
            "case {case:?}: the fieldId must be a server-minted opaque id, got {field_id:?}"
        );
        assert_eq!(
            string_at(&response, "fieldArn", &case),
            format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/field/{field_id}"),
            "case {case:?}: fieldArn must address the stored field, got {response:?}"
        );
        created.push((*field_type, field_id));
    }
    created
}

/// Create a template requiring `field_ids`, returning its id.
pub async fn template(
    emulator: &Emulator,
    domain_id: &str,
    name: &str,
    field_ids: &[String],
) -> String {
    let required: Vec<Value> = field_ids
        .iter()
        .map(|id| json!({ "fieldId": id }))
        .collect();
    let response = emulator
        .ok(
            name,
            "POST",
            &format!("/domains/{domain_id}/templates"),
            json!({ "name": name, "requiredFields": required }),
        )
        .await;
    string_at(&response, "templateId", name)
}

/// `GetCase` for `ids`, returning the `fields` list verbatim.
pub async fn get_case_fields(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    case_id: &str,
    ids: &[String],
) -> Vec<Value> {
    let requested: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
    let response = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({ "fields": requested }),
        )
        .await;
    response
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: GetCase returned no fields list: {response:?}"))
        .clone()
}

/// `true` when `id` is a server-minted resource id: the 8-4-4-4-12 lowercase
/// hex form the real service's ids take, carrying nothing from the request.
pub fn is_minted_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
}
