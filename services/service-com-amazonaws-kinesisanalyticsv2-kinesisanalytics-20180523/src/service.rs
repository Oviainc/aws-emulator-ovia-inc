// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

pub mod request_layer;

/// The enumerated value sets this service NAMES differently from the enum its public model
/// publishes, as (what the model declares, what the service names) pairs.
///
/// A refusal of a value outside an enumerated member states the admissible set, and the set
/// it states is the one the SERVICE's own validator enumerates for that shape — which a
/// public model revision can declare more values than, because the model is published from
/// the SDK's view of the API and gains a value as soon as the API documents it. Where the two
/// differ the service's set is carried here and replaces the model's; a set this table does
/// not name is stated as the model declares it, which is what the framework already renders.
///
/// SOURCE, for adding an entry: the bracketed set in the service's OWN refusal of a value
/// outside that enum. That is service data about the enum, so an entry is added only when
/// such a statement is in hand and it disagrees with the model's declaration — which is why
/// this table is short rather than one entry per enum of the model.
///
/// An entry is matched by its MEMBERSHIP — the values the model declares, in any order — so
/// it answers for the SHAPE rather than for one member referencing it: every member targeting
/// that enum, in any operation (`RuntimeEnvironment` is reached by the runtime a create names
/// and by the runtime an update moves an application to), is stated the same way. The set
/// stated is only what the refusal NAMES: which values the service accepts is the model's
/// enum, unchanged, so a value this table leaves out is still taken by the boundary.
const NAMED_BY_THE_SERVICE: [(&[&str], &[&str]); 1] = [(
    // `RuntimeEnvironment` (`smithy_query(query="shape", shape="RuntimeEnvironment")`), whose
    // refusals name the runtime families the service enumerates — the SQL runtime, the Flink
    // runtimes up to 1_15 and the Zeppelin-Flink notebook runtimes — and not the later Flink
    // releases the published model also declares.
    &[
        "SQL-1_0",
        "FLINK-1_6",
        "FLINK-1_8",
        "ZEPPELIN-FLINK-1_0",
        "FLINK-1_11",
        "FLINK-1_13",
        "ZEPPELIN-FLINK-2_0",
        "FLINK-1_15",
        "ZEPPELIN-FLINK-3_0",
        "FLINK-1_18",
        "FLINK-1_19",
        "FLINK-1_20",
        "FLINK-2_2",
        "FLINK-2_3",
    ],
    &[
        "FLINK-1_11",
        "FLINK-1_13",
        "FLINK-1_15",
        "FLINK-1_6",
        "FLINK-1_8",
        "SQL-1_0",
        "ZEPPELIN-FLINK-1_0",
        "ZEPPELIN-FLINK-2_0",
        "ZEPPELIN-FLINK-3_0",
    ],
)];

/// The admissible set a refusal states for an enumerated member whose model-declared set is
/// `declared`: the set this service names for it ([`NAMED_BY_THE_SERVICE`]), or the model's
/// own set when the two agree.
///
/// The match is on membership and size, never on position, so a model revision that reordered
/// its own declaration of the same enum is still stated as the service names it.
fn named_value_set(declared: &[String]) -> Vec<String> {
    for (model_names, service_names) in NAMED_BY_THE_SERVICE {
        let same_set = declared.len() == model_names.len()
            && model_names
                .iter()
                .all(|named| declared.iter().any(|value| value == named));
        if same_set {
            return service_names.iter().map(|value| (*value).to_string()).collect();
        }
    }
    declared.to_vec()
}

/// `refusal` with every enumerated value set stated as this service states it — the set the
/// service names ([`named_value_set`]), in LEXICOGRAPHIC order — and everything else
/// — the member paths, the values echoed, every other requirement — untouched.
///
/// A refusal of a value outside an enumerated type names the values that type admits, and
/// the order it names them in is the service's rather than the model's: the order the model
/// happens to declare an enum's members in is an artefact of how the model was written
/// (`RuntimeEnvironment` declares its runtimes in release order), while this service states
/// such a set sorted. The re-ordering itself is `service_common`'s one statement of that
/// rule, and it is applied to the violations as they arrive, so it answers for every enum of
/// this model and for every member targeting one, in any operation, rather than for a table
/// of enums written out here.
fn as_stated_refusal(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(violations.iter().map(as_stated).collect())
}

/// One violation as this service states it: an enumerated member's admissible set is the set
/// the service names, stated sorted, and every other constraint class is returned unchanged.
///
/// The CLAUSE stays the framework's throughout — this only decides what goes into the
/// bracketed set and in what order, so the member path, the echoed value and the
/// requirement's wording keep coming from the one place that writes them.
fn as_stated(violation: &Violation) -> Violation {
    let named = match &violation.kind {
        ViolationKind::EnumValue(declared) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::EnumValue(named_value_set(declared)),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        },
        _ => violation.clone(),
    };
    service_common::validation_message::in_sorted_enum_order(&named)
}

/// The `com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// What a request whose body carries a value no modelled timestamp member can be read
    /// from is served by, instead of the operation it named: see
    /// [`request_layer`]. Holds no state, so one value serves every such request.
    unreadable_body: request_layer::UnreadableBody,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            unreadable_body: request_layer::UnreadableBody,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service is the 2018-05-23 API version of Kinesis Analytics, and AWS serves
        // it behind the SAME endpoint and the same SigV4 signing name as the 2015-08-14
        // version (`endpointPrefix`/`sigv4Name` `kinesisanalytics` in both models). The
        // router resolves a request's routing key from the credential scope, so both
        // versions see that one key: claiming on the signing name alone makes both of them
        // claim every request, which the router answers `500 AmbiguousService` — a code no
        // AWS model carries — instead of serving either.
        //
        // What names the version is the `awsJson1_1` wire form itself:
        // `X-Amz-Target: KinesisAnalytics_20180523.<Operation>` for this service and
        // `KinesisAnalytics_20150814.<Operation>` for its peer. So the claim is the one
        // rule `service-common` states for an endpoint serving several API versions — the
        // routing key AND a target naming THIS service shape, API-version suffix included,
        // since that suffix is the only difference between the two names.
        service_common::claims_versioned_rpc_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            self.entry.identity().shape_id(),
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // An `awsJson` target names its operation in the segment after its LAST dot,
        // whatever prefix stands in front of it — and some SDKs write that prefix as the
        // fully qualified Coral name, which carries dots of its own
        // (`com.amazonaws.kinesisanalyticsv2.KinesisAnalytics_20180523.<Operation>`). The
        // framework resolves a routing key from everything after the FIRST dot, so a
        // dotted prefix resolves a key no modelled operation is wired under and the
        // request is refused before any handler runs, even though `can_handle` above
        // (which reads the same target by the same rule) claimed it.
        //
        // So the operation is resolved here through `service_common::rpc_target`, this
        // emulator's one statement of that rule. It is the same answer for the short
        // prefix an SDK normally sends, so one rule serves both forms; a target naming
        // another service, or an operation this model does not bind, resolves to `None`
        // and is left to the generated routing table and its protocol-correct refusal.
        let operation =
            service_common::rpc_target::requested_operation(req, self.entry.identity().shape_id())?;

        // Deserialization precedes every later layer, so a value the protocol cannot read
        // into the modelled member it is bound to is refused BEFORE the operation runs and
        // before anything is mutated: an `epoch-seconds` timestamp carrying a quoted number
        // is such a value, and `crate::service::request_layer` states that rule for every
        // timestamp member of this model.
        if request_layer::unreadable_timestamp(operation, request_layer::request_body(req))
            .is_some()
        {
            return Some(&self.unreadable_body);
        }

        self.entry.routing_table().get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound or an enumerated set)
        // before any handler runs. This service is `awsJson1_1`, where AWS answers such a
        // refusal as `400 ValidationException` with the `N validation error(s) detected:
        // …` envelope — the envelope, the member paths and the requirement sentences the
        // framework already renders, which is why this delegates to the ONE shared
        // statement of them (`service_common::validation_message`) instead of writing a
        // second one here. The `@pattern` rendering passed to it is the identity, so a
        // pattern is quoted back as this model writes it, anchors included.
        //
        // What this service chooses is the enumerated value SET that is stated and the ORDER
        // it is stated in ([`as_stated_refusal`]): the set the service names for that shape
        // ([`NAMED_BY_THE_SERVICE`]), which is not always the set the public model declares,
        // sorted rather than left in the model's declaration order.
        service_common::validation_message::aws_validation_refusal_with(
            &as_stated_refusal(refusal),
            |pattern| pattern.to_string(),
        )
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
