// SPDX-License-Identifier: CC0-1.0

//! Every `crate::ops::<Name>` path of this service names exactly one item, and each
//! operation keeps its own handler.
//!
//! The rule: a name is published once under `crate::ops`, whichever generated glob
//! (`types::*` or `<snake_op>::*`) carries it, so no `crate::ops::<Name>` depends on the
//! order the ops-module generator emits those globs in. `crate::ops::<ShapeName>` names
//! the MODELLED SHAPE — the generated service declaration is expanded with
//! `types = "crate::ops"`, so that is the path the wire machinery resolves every
//! input/output shape through — and an operation's handler struct is reached at
//! `crate::ops::<snake_op>::<Op>` as well as through its glob.
//!
//! Over the wire, the answer that identifies the handler a request reached is the
//! scaffold's 501 for an operation nobody has written yet, and that operation's own
//! modelled error for an implemented one (see [`Reached`]).
//!
//! This model is one that could break the rule twice: `UpdateBridgeOutput` and
//! `UpdateFlowOutput` are operations AND the `<Op>Output` form of the `UpdateBridge` /
//! `UpdateFlow` operations' output structs. It does not, because those output structs
//! carry the model's own output shape names (`UpdateBridgeResponse`,
//! `UpdateFlowResponse`). The rows below span the class rather than those two names: both
//! operations whose names are at risk, both operations whose output structs yield them,
//! an `…Output` operation with no such tension (`RemoveFlowOutput`) and an operation name
//! with none at all (`UpdateBridgeSource`), each driven over the wire on its own modelled
//! route.

use std::any::{type_name, TypeId};
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_mediaconnect_mediaconnect::{ops, EmulatedService};
use smithy_server_runtime::{MultiServiceRouter, Operation};

/// The engine the service and all of its operations are constructed from.
type Eng = engine::InMemoryEngine;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`mediaconnect`, per `OPERATIONS.md` "Wire protocol"): that scope is how the
/// router picks the service which claims a request. The signature itself is never
/// verified here — [`Emulator`] turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/mediaconnect/aws4_request, SignedHeaders=host, Signature=not-verified";

/// A bridge ARN matching the `BridgeArn` member pattern
/// `^arn:.+:mediaconnect.+:bridge:.+$` (`smithy_query(query="operation")`).
const BRIDGE_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:bridge:collision-case";
/// A flow ARN matching the `FlowArn` shape's pattern
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`
/// (`smithy_query(query="shape",shape="FlowArn")`).
const FLOW_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-23456789abcdef:collision-case";
/// An output ARN matching the `OutputArn` member pattern
/// `^arn:.+:mediaconnect.+:output:.+$`.
const OUTPUT_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:output:collision-case";

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
        // The names at risk belong to their operations, and to nothing else.
        row::<
            ops::UpdateBridgeOutput<Eng>,
            ops::update_bridge_output::UpdateBridgeOutput<Eng>,
        >("ops::UpdateBridgeOutput is the UpdateBridgeOutput operation struct"),
        row::<ops::UpdateFlowOutput<Eng>, ops::update_flow_output::UpdateFlowOutput<Eng>>(
            "ops::UpdateFlowOutput is the UpdateFlowOutput operation struct",
        ),
        // So each of those operations' output shapes is published under the model's own
        // output shape name, and that is the binding the service declaration resolves.
        row::<
            <ops::update_bridge::UpdateBridge<Eng> as Operation>::Output,
            ops::UpdateBridgeResponse,
        >("UpdateBridge's output shape is ops::UpdateBridgeResponse"),
        row::<<ops::update_flow::UpdateFlow<Eng> as Operation>::Output, ops::UpdateFlowResponse>(
            "UpdateFlow's output shape is ops::UpdateFlowResponse",
        ),
        // An `…Output` operation name with no such tension (there is no `RemoveFlow`
        // operation), and a name with none at all: both still reach the operation struct
        // through the ops module.
        row::<ops::RemoveFlowOutput<Eng>, ops::remove_flow_output::RemoveFlowOutput<Eng>>(
            "ops::RemoveFlowOutput is the RemoveFlowOutput operation struct",
        ),
        row::<ops::UpdateBridgeSource<Eng>, ops::update_bridge_source::UpdateBridgeSource<Eng>>(
            "ops::UpdateBridgeSource is the UpdateBridgeSource operation struct",
        ),
        // Each of those operations' own input and output shapes keep their own bindings.
        row::<
            <ops::update_bridge_output::UpdateBridgeOutput<Eng> as Operation>::Input,
            ops::UpdateBridgeOutputInput,
        >("UpdateBridgeOutput's input shape is ops::UpdateBridgeOutputInput"),
        row::<
            <ops::update_bridge_output::UpdateBridgeOutput<Eng> as Operation>::Output,
            ops::UpdateBridgeOutputOutput,
        >("UpdateBridgeOutput's output shape is ops::UpdateBridgeOutputOutput"),
        row::<
            <ops::update_flow_output::UpdateFlowOutput<Eng> as Operation>::Input,
            ops::UpdateFlowOutputInput,
        >("UpdateFlowOutput's input shape is ops::UpdateFlowOutputInput"),
        row::<
            <ops::update_flow_output::UpdateFlowOutput<Eng> as Operation>::Output,
            ops::UpdateFlowOutputOutput,
        >("UpdateFlowOutput's output shape is ops::UpdateFlowOutputOutput"),
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

    // Each pair of operations whose names are one a prefix of the other answers with
    // different output shapes: neither glob shadowed the other's.
    assert_ne!(
        TypeId::of::<<ops::update_bridge::UpdateBridge<Eng> as Operation>::Output>(),
        TypeId::of::<<ops::update_bridge_output::UpdateBridgeOutput<Eng> as Operation>::Output>(),
        "case {:?}: the two operations must keep distinct output shapes",
        "UpdateBridge vs UpdateBridgeOutput"
    );
    assert_ne!(
        TypeId::of::<<ops::update_flow::UpdateFlow<Eng> as Operation>::Output>(),
        TypeId::of::<<ops::update_flow_output::UpdateFlowOutput<Eng> as Operation>::Output>(),
        "case {:?}: the two operations must keep distinct output shapes",
        "UpdateFlow vs UpdateFlowOutput"
    );

    // Every operation reports its own wire name.
    for (case, actual, expected) in [
        (
            "UpdateBridge",
            <ops::update_bridge::UpdateBridge<Eng> as Operation>::NAME,
            "UpdateBridge",
        ),
        (
            "UpdateBridgeOutput",
            <ops::update_bridge_output::UpdateBridgeOutput<Eng> as Operation>::NAME,
            "UpdateBridgeOutput",
        ),
        (
            "UpdateFlow",
            <ops::update_flow::UpdateFlow<Eng> as Operation>::NAME,
            "UpdateFlow",
        ),
        (
            "UpdateFlowOutput",
            <ops::update_flow_output::UpdateFlowOutput<Eng> as Operation>::NAME,
            "UpdateFlowOutput",
        ),
        (
            "RemoveFlowOutput",
            <ops::remove_flow_output::RemoveFlowOutput<Eng> as Operation>::NAME,
            "RemoveFlowOutput",
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

/// What identifies the handler a request reached.
///
/// A SCAFFOLDED operation answers HTTP 501 `NotImplemented` with a message naming the
/// operation. An IMPLEMENTED one answers its own modelled error for the resource the
/// request names — every ARN below names a flow or bridge no store holds — so the row
/// carries the status and wire code that answer has.
enum Reached {
    ScaffoldedStub,
    ModelledError { status: u16, code: &'static str },
}

/// One VALID request per operation — method, URI and body from each operation's own
/// `@http` binding (`smithy_query(query="operation")`), valid so the boundary validator
/// passes it and it reaches that operation's own handler.
fn dispatch_table() -> Vec<(&'static str, &'static str, String, &'static str, Reached)> {
    vec![
        (
            // Implemented: it answers the modelled `NotFoundException` (404) for a bridge
            // ARN this store does not hold.
            "UpdateBridge",
            "PUT",
            format!("/v1/bridges/{BRIDGE_ARN}"),
            "{}",
            Reached::ModelledError {
                status: 404,
                code: "NotFoundException",
            },
        ),
        (
            "UpdateBridgeOutput",
            "PUT",
            format!("/v1/bridges/{BRIDGE_ARN}/outputs/output-1"),
            "{}",
            Reached::ScaffoldedStub,
        ),
        (
            // Implemented: it answers the modelled `NotFoundException` (404) for a flow
            // ARN this store does not hold.
            "UpdateFlow",
            "PUT",
            format!("/v1/flows/{FLOW_ARN}"),
            "{}",
            Reached::ModelledError {
                status: 404,
                code: "NotFoundException",
            },
        ),
        (
            // Implemented: it answers the modelled `NotFoundException` (404) for a flow
            // ARN this store does not hold.
            "UpdateFlowOutput",
            "PUT",
            format!("/v1/flows/{FLOW_ARN}/outputs/{OUTPUT_ARN}"),
            "{}",
            Reached::ModelledError {
                status: 404,
                code: "NotFoundException",
            },
        ),
        (
            // Implemented: it answers the modelled `NotFoundException` (404) for a bridge
            // ARN this store does not hold.
            "UpdateBridgeSource",
            "PUT",
            format!("/v1/bridges/{BRIDGE_ARN}/sources/source-1"),
            "{}",
            Reached::ModelledError {
                status: 404,
                code: "NotFoundException",
            },
        ),
        (
            // Implemented: it answers the modelled `NotFoundException` (404) for a flow
            // ARN this store does not hold.
            "RemoveFlowOutput",
            "DELETE",
            format!("/v1/flows/{FLOW_ARN}/outputs/{OUTPUT_ARN}"),
            "",
            Reached::ModelledError {
                status: 404,
                code: "NotFoundException",
            },
        ),
    ]
}

#[tokio::test]
async fn every_operation_dispatches_to_the_handler_that_bears_its_name() {
    let emulator = Emulator::new();

    for (operation, method, path, body, reached) in dispatch_table() {
        let (status, code, parsed) = emulator.send(method, &path, body).await;
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        match reached {
            Reached::ScaffoldedStub => {
                assert_eq!(
                    status, 501,
                    "case {operation:?} ({method} {path}): expected HTTP 501 from the \
                     scaffolded handler, got {status} with {parsed:?}"
                );
                assert!(
                    code.ends_with("NotImplemented"),
                    "case {operation:?}: expected the error code NotImplemented, got \
                     {code:?} with {parsed:?}"
                );
                assert!(
                    message.contains(&format!("operation {operation} is")),
                    "case {operation:?}: expected the 501 to name {operation}, got {message:?}"
                );
            }
            Reached::ModelledError {
                status: expected_status,
                code: expected_code,
            } => {
                assert_eq!(
                    (status, code.as_str()),
                    (expected_status, expected_code),
                    "case {operation:?} ({method} {path}): expected the implemented \
                     handler's {expected_status} {expected_code}, got {status} {code:?} \
                     with {parsed:?}"
                );
                assert!(
                    !message.contains("is scaffolded"),
                    "case {operation:?}: an implemented handler never answers the \
                     scaffold's wording, got {message:?}"
                );
            }
        }
    }
}
