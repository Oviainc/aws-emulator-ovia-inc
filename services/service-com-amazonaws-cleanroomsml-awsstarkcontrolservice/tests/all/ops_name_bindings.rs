// SPDX-License-Identifier: CC0-1.0

//! Every `crate::ops::<Name>` path of this service names exactly one item.
//!
//! The rule: a name is published once under `crate::ops`, whichever generated glob
//! (`types::*` or `<snake_op>::*`) carries it, so no `crate::ops::<Name>` depends on the
//! order the ops-module generator emits those globs in. `crate::ops::<ShapeName>` names
//! the MODELLED SHAPE — the generated service declaration is expanded with
//! `types = "crate::ops"`, so that is the path the wire machinery resolves every
//! input/output shape through — and an operation's handler struct is reached at
//! `crate::ops::<snake_op>::<Op>` as well as through its glob.
//!
//! This service used to carry a crate-level `allow(ambiguous_glob_reexports)` on
//! `src/ops/mod.rs` for a name no two globs publish: `DeleteTrainedModelOutput`
//! is an operation here, but the model declares no `DeleteTrainedModel` operation, so no
//! generated shape bears that name and the glob re-exports do not collide. These rows pin
//! that — the name binds to the OPERATION struct, and its own input/output shapes keep
//! their own names — so the allow stayed removed.

use std::any::{type_name, TypeId};
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::{ops, EmulatedService};
use smithy_server_runtime::{MultiServiceRouter, Operation};

/// The engine the service and all of its operations are constructed from.
type Eng = engine::InMemoryEngine;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`cleanrooms-ml`, per `OPERATIONS.md` "Wire protocol"): that scope is how the
/// router picks the service which claims a request. The signature itself is never
/// verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/cleanrooms-ml/aws4_request, SignedHeaders=host, Signature=not-verified";

/// A membership id matching the `UUID` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="operation",operation="GetTrainedModel")`).
const MEMBERSHIP: &str = "11111111-2222-3333-4444-555555555555";

/// One expectation about which item a `crate::ops::…` path names.
struct TypeRow {
    case: &'static str,
    actual: TypeId,
    actual_name: &'static str,
    expected: TypeId,
    expected_name: &'static str,
}

/// `Actual` is the type a `crate::ops::…` path resolves to; `Expected` is the item it
/// must be.
fn row<Actual: ?Sized + 'static, Expected: ?Sized + 'static>(case: &'static str) -> TypeRow {
    TypeRow {
        case,
        actual: TypeId::of::<Actual>(),
        actual_name: type_name::<Actual>(),
        expected: TypeId::of::<Expected>(),
        expected_name: type_name::<Expected>(),
    }
}

#[test]
fn ops_paths_name_exactly_one_item_each() {
    let rows = [
        // An `…Output` operation name that no shape in this model shares: the path names
        // the operation struct, which is why nothing here ever needed the lint allowed.
        row::<
            ops::DeleteTrainedModelOutput<Eng>,
            ops::delete_trained_model_output::DeleteTrainedModelOutput<Eng>,
        >("ops::DeleteTrainedModelOutput is the DeleteTrainedModelOutput operation struct"),
        row::<ops::GetTrainedModel<Eng>, ops::get_trained_model::GetTrainedModel<Eng>>(
            "ops::GetTrainedModel is the GetTrainedModel operation struct",
        ),
        // That operation's own input and output shapes keep their own bindings.
        row::<
            <ops::delete_trained_model_output::DeleteTrainedModelOutput<Eng> as Operation>::Input,
            ops::DeleteTrainedModelOutputInput,
        >("DeleteTrainedModelOutput's input shape is ops::DeleteTrainedModelOutputInput"),
        row::<
            <ops::delete_trained_model_output::DeleteTrainedModelOutput<Eng> as Operation>::Output,
            ops::DeleteTrainedModelOutputOutput,
        >("DeleteTrainedModelOutput's output shape is ops::DeleteTrainedModelOutputOutput"),
    ];

    for TypeRow {
        case,
        actual,
        actual_name,
        expected,
        expected_name,
    } in rows
    {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected_name}, got {actual_name}"
        );
    }

    for (case, actual, expected) in [
        (
            "DeleteTrainedModelOutput",
            <ops::delete_trained_model_output::DeleteTrainedModelOutput<Eng> as Operation>::NAME,
            "DeleteTrainedModelOutput",
        ),
        (
            "GetTrainedModel",
            <ops::get_trained_model::GetTrainedModel<Eng> as Operation>::NAME,
            "GetTrainedModel",
        ),
        (
            "ListTrainedModels",
            <ops::list_trained_models::ListTrainedModels<Eng> as Operation>::NAME,
            "ListTrainedModels",
        ),
    ] {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected Operation::NAME {expected:?}, got {actual:?}"
        );
    }
}

/// A router with only this service mounted — one emulator "session".
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::new(Eng::new())));
        Self { router }
    }

    /// Send one restJson1 request; returns the status, the wire error code and the parsed
    /// body.
    async fn send(&self, method: &str, path: &str, body: &str) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let code = if header_code.is_empty() {
            parsed
                .get("__type")
                .or_else(|| parsed.get("code"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        (status, code, parsed)
    }
}

/// One VALID request per operation — method and URI from each operation's own `@http`
/// binding (`smithy_query(query="operation")`). `DeleteTrainedModelOutput` and
/// `GetTrainedModel` share one URI and differ only in method, so the pair also shows that
/// two operations whose names overlap are dispatched apart.
fn dispatch_table() -> Vec<(&'static str, &'static str, String, &'static str)> {
    // A non-greedy `{trainedModelArn}` label carries its `/` percent-encoded, the way an
    // SDK serializes an ARN into one path segment.
    let trained_model_arn = format!(
        "arn:aws:cleanrooms-ml:us-east-1:123456789012:membership%2F{MEMBERSHIP}%2Ftrained-model%2Fcollision-case"
    );
    vec![
        (
            "DeleteTrainedModelOutput",
            "DELETE",
            format!("/memberships/{MEMBERSHIP}/trained-models/{trained_model_arn}"),
            "",
        ),
        (
            "GetTrainedModel",
            "GET",
            format!("/memberships/{MEMBERSHIP}/trained-models/{trained_model_arn}"),
            "",
        ),
        (
            "ListTrainedModels",
            "GET",
            format!("/memberships/{MEMBERSHIP}/trained-models"),
            "",
        ),
    ]
}

#[tokio::test]
async fn every_operation_dispatches_to_the_handler_that_bears_its_name() {
    let emulator = Emulator::new();

    for (operation, method, path, body) in dispatch_table() {
        let (status, code, parsed) = emulator.send(method, &path, body).await;

        // Each of these operations is still a scaffold, so the answer that identifies the
        // handler the request reached is its 501 — which names the operation.
        assert_eq!(
            status, 501,
            "case {operation:?} ({method} {path}): expected HTTP 501 from the scaffolded \
             handler, got {status} with {parsed:?}"
        );
        assert!(
            code.ends_with("NotImplemented"),
            "case {operation:?}: expected the error code NotImplemented, got {code:?} with {parsed:?}"
        );

        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&format!("operation {operation} is")),
            "case {operation:?}: expected the 501 to name {operation}, got {message:?}"
        );
    }
}
