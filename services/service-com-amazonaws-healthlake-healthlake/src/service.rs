// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-healthlake-healthlake` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it for two things. First, HealthLake serves three
//!   surfaces that are not awsJson1_0 RPC calls and carry no `X-Amz-Target` at all —
//!   a data store's FHIR REST data plane ([`fhir_rest`], on the path a
//!   `DatastoreEndpoint` publishes), the data-transformation endpoint's
//!   `/validate-source` check ([`validate_source`]) and that endpoint's REST tagging
//!   surface ([`rest_tagging`], `/tags` and `/untag`); the first is not a modelled
//!   operation at all and the others are modelled outside the closure of the service
//!   shape this crate serves, so the generated routing table cannot name any of them,
//!   and this seam routes them by
//!   HTTP method and path, which is how a REST surface is addressed. Second, every
//!   other request is handed to the input-validation gate described below, which lets
//!   the generated routing table serve it from there.
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
//!
//! ## Why this file validates input
//!
//! HealthLake models `ValidationException` with one member, `Message` (target
//! `HealthLakeString`, length 0..10000), and it is a COMMON error of every operation
//! (`smithy_query(query="service_info")` -> `common_errors: ["ValidationException"]`).
//! An SDK and the CLI surface a refusal through exactly that member, so a 400 whose body
//! carries no `Message` reaches the caller with an empty message.
//!
//! The framework's pre-dispatch envelope cannot set it: it renders a refusal as a
//! generic `{"__type":"ValidationException","message":…}` — at that point the refusal is
//! a protocol-level fact rather than one of this service's error SHAPES, and the only
//! seam over it ([`Service::override_refusal`]) chooses a code and a sentence, not a
//! body — so a client deserialising the modelled shape finds no `Message` at all.
//!
//! So this service answers a constraint refusal from its own modelled shape, through
//! [`ModeledInputValidation`]: a gate [`Service::override_operation`] puts in front of
//! every modelled operation, which asks the framework's OWN constraint walk
//! (`aws_smithy_schema_server::validate`, the same call the centralized sequence makes)
//! what the request violates and renders those violations with the framework's own
//! `envelope` — the `N validation error(s) detected: Value '…' at '…' failed to satisfy
//! constraint: …` sentence AWS itself returns — into the modelled `Message`, and into
//! the framework's lowercase `message` beside it, which is the key this service's
//! data-transformation endpoint carries that same sentence under. Any request
//! the model accepts is handed straight to the generated operation. The rules enforced
//! are therefore exactly the model's — nothing about a constraint is restated here —
//! with two reconciliations the recorded wire requires, each in ONE place: the
//! `SourceFormat` clause names the value set the service's own refusal REPORTS, which
//! the snapshot's enum lags ([`reported_violations`]), and a member whose `@pattern`
//! requires an ARN that was handed a value which is not an ARN at all is answered by
//! the ARN parser rather than by the member's pattern ([`arn_syntax_message`]).
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the
//! per-operation telemetry the centralized sequence records — a request the gate
//! refuses never enters that sequence, so it is counted nowhere, exactly as a request
//! refused before dispatch is.

pub mod fhir_rest;
pub mod non_rpc;
pub mod rest_tagging;
pub mod synthea;
pub mod validate_source;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema_server::{validate, ProtocolError};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};

/// The error code this service refuses a request that failed a modelled
/// constraint with: `ValidationException`, the service-common client error the
/// model declares at HTTP 400 (`smithy_query(query="service_info")` lists it
/// among the common errors, and every operation carries it).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The status a `ValidationException` is served with: a modelled CLIENT fault on this
/// service's protocol, chosen where every other modelled error's status is chosen
/// ([`crate::scaffolded::wire_http_status`], fed the shape's own `@httpError(400)`).
const VALIDATION_EXCEPTION_MODELED_STATUS: u16 = 400;

/// How this service states a set of constraint violations the framework refused
/// before any handler ran.
///
/// The code is [`VALIDATION_EXCEPTION_CODE`] and the message is [`refusal_message`].
///
/// This is the FALLBACK wording, for a refusal that reaches
/// [`Service::override_refusal`]: a constraint refusal of an operation this crate serves
/// is answered earlier, and from the MODELLED error shape, by
/// [`ModeledInputValidation`] — which words it with the same
/// [`refusal_message`], so the two cannot say different things.
fn refused_for(violations: &[Violation]) -> Refused {
    Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: refusal_message(&reported_violations(violations.to_vec())),
    }
}

/// The member path of a `SourceFormat` member, as a violation path spells a member
/// name (lowerCamel; the member is spelled the same in every input of this service
/// that carries it).
const SOURCE_FORMAT_PATH: &str = "sourceFormat";

/// The prefix an ARN starts with — and the prefix the `@pattern` of a member that
/// requires an ARN starts with, which is how such a member is recognised.
const ARN_PREFIX: &str = "arn:";

/// How this service words a request the model refused before any handler ran.
///
/// The `N validation error(s) detected` envelope ([`validation_envelope`]), EXCEPT
/// where a member whose `@pattern` requires an ARN was handed a value that is not an
/// ARN at all: the service PARSES such a value before any member pattern is checked,
/// so the caller is answered with the ARN parser's own refusal — see
/// [`arn_syntax_message`].
///
/// `pub(crate)` because the non-RPC surfaces and any handler re-stating a modelled
/// constraint of its own have to answer in the SAME words.
pub(crate) fn refusal_message(violations: &[Violation]) -> String {
    arn_syntax_message(violations).unwrap_or_else(|| validation_envelope(violations))
}

/// The ARN parser's refusal for the first violation of an ARN-shaped member whose
/// value does not begin with `arn:`, or `None` when no violation is one.
///
/// A member is ARN-shaped when the `@pattern` it failed requires the value to start
/// with [`ARN_PREFIX`], which is read off the violated pattern itself — so every ARN
/// member of this service is covered and no member list is restated here. The
/// rejected value is the string the parser could not parse, so it is echoed as the
/// parser echoes it (already bounded: the framework elides an echoed value).
///
/// The parse is what the service does with a WELL-FORMED request's ARN, so this
/// answers only when that pattern is the sole rule the request broke: a value that
/// also breaks another declared rule — its `@length`, say — is refused by that rule,
/// which the validation framework answers before any ARN is parsed.
fn arn_syntax_message(violations: &[Violation]) -> Option<String> {
    let [violation] = violations else {
        return None;
    };
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    let requires_arn = unanchored(pattern).starts_with(ARN_PREFIX);
    let value = violation.value.as_deref()?;
    (requires_arn && !value.starts_with(ARN_PREFIX)).then(|| {
        format!("com.amazon.arn.ARNSyntaxException: ARNs must start with '{ARN_PREFIX}': {value}")
    })
}

/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`, each clause as [`clause_for`] words it.
///
/// `pub(crate)` because a handler re-stating a modelled constraint of its own
/// has to answer in the SAME words as the boundary validator; see
/// [`crate::ops::create_fhir_datastore`].
pub(crate) fn validation_envelope(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(clause_for).collect();
    match clauses.len() {
        // The framework only calls this with at least one violation; an empty
        // set still has to read as a refusal rather than as an empty sentence.
        0 => "1 validation error detected".to_string(),
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One violation as this service states it.
///
/// Two classes state the member and the requirement ALONE, with no echo of what the
/// caller sent:
///
/// * a value outside an enumerated type's value set — an unknown enum name never
///   becomes a value of the modelled type, so there is no value for the refusal to
///   quote, and what the caller needs is the permitted set; and
/// * a value outside a member's `@pattern` — the value is the thing the pattern
///   rejected, and what the caller needs is the pattern, stated as the member
///   declares it but WITHOUT the `^`/`$` anchors, which say where the pattern is
///   applied rather than what it admits.
///
/// Every other class (length, range, uniqueness, absence) keeps the framework's own
/// clause, which quotes the offending value as AWS does.
fn clause_for(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::EnumValue(allowed) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy enum value set: [{}]",
            violation.path,
            allowed.join(", ")
        ),
        ViolationKind::Pattern(pattern) => format!(
            "Value at '{}' failed to satisfy constraint: Member must satisfy regular expression \
             pattern: {}",
            violation.path,
            unanchored(pattern)
        ),
        // `ViolationKind` is `#[non_exhaustive]`: anything else — including a
        // class added later — keeps the framework's wording rather than being
        // dropped or reworded on no evidence.
        _ => violation.message(),
    }
}

/// `pattern` without the `^` and `$` a Smithy `@pattern` anchors it with.
///
/// The pair is dropped only when BOTH ends carry it — what the service states is the
/// expression the value as a whole must match, and an expression anchored at one end
/// only does not describe that — which is the rule the shared renderer in
/// `services/common` states for the services already migrated to it. This service
/// states it locally, as its sibling services still do.
fn unanchored(pattern: &str) -> &str {
    match pattern.strip_prefix('^').and_then(|body| body.strip_suffix('$')) {
        Some(inner) => inner,
        None => pattern,
    }
}

/// `violations` with the `SourceFormat` enum clause stating the value set this
/// service REPORTS ([`crate::state::REPORTED_SOURCE_FORMATS`]).
///
/// The on-disk model snapshot's `SourceFormat` enum lags the service, so the clause
/// the framework words from the snapshot names fewer formats than the service's own
/// refusal does, and a caller correcting its request from the refusal would not learn
/// what to send. Only the SENTENCE is reconciled: the constraint enforced is still
/// the model's, walked by the framework.
fn reported_violations(violations: Vec<Violation>) -> Vec<Violation> {
    violations
        .into_iter()
        .map(|mut violation| {
            if is_source_format_enum(&violation) {
                violation.kind = ViolationKind::EnumValue(
                    crate::state::REPORTED_SOURCE_FORMATS
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect(),
                );
            }
            violation
        })
        .collect()
}

/// Whether `violation` is a value outside the `SourceFormat` member's enum.
fn is_source_format_enum(violation: &Violation) -> bool {
    violation.path == SOURCE_FORMAT_PATH
        && matches!(violation.kind, ViolationKind::EnumValue(_))
}

/// The message this service answers a value outside `allowed` at the member
/// `path` with, for a handler that re-states an enum constraint the boundary
/// validator has already applied. Worded by [`clause_for`], so the handler's
/// answer and the validator's are the same sentence.
pub(crate) fn enum_violation_message(path: &str, allowed: &[&str]) -> String {
    validation_envelope(&[Violation {
        path: path.to_string(),
        kind: ViolationKind::EnumValue(allowed.iter().map(|value| value.to_string()).collect()),
        // Deliberately absent: this service's enum clause quotes no value.
        value: None,
        sensitive: false,
    }])
}

/// The body a constraint refusal of this service carries: the modelled
/// `ValidationException` and, beside it, the same sentence under the framework's
/// lowercase `message` key.
///
/// The modelled `ValidationException` of this service declares one member, `Message`
/// (`smithy_query(query="shape",shape="ValidationException")`), and that is what an
/// SDK deserialising the modelled shape reads. The recorded wire shows a constraint
/// refusal arriving in the modelled `Message` on this service's control-plane
/// endpoint and in the framework's lowercase `message` on its data-transformation
/// endpoint, which this emulator serves from the same listener. So a refusal carries
/// ONE sentence under BOTH keys: every client path reads the reason, and the two can
/// never state different ones. The lowercase member is the one
/// [`crate::scaffolded::VALIDATION_EXCEPTION_MESSAGE`] declares, so this crate holds
/// a single schema for it.
struct ConstraintRefusal {
    /// The modelled shape, holding the sentence in its `Message` member.
    modelled: crate::ops::types::ValidationException,
}

impl ConstraintRefusal {
    /// The refusal that states `message`.
    fn new(message: String) -> Self {
        Self {
            modelled: crate::ops::types::ValidationException {
                message: Some(message),
            },
        }
    }

    /// The sentence, as both members carry it.
    fn message(&self) -> &str {
        self.modelled.message.as_deref().unwrap_or_default()
    }
}

impl SerializableStruct for ConstraintRefusal {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        SerializableStruct::serialize_members(&self.modelled, serializer)?;
        serializer.write_string(
            &crate::scaffolded::VALIDATION_EXCEPTION_MESSAGE,
            self.message(),
        )
    }
}

/// The `com-amazonaws-healthlake-healthlake` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves a data store's FHIR REST data plane — the wire its
    /// `DatastoreEndpoint` advertises. Held on the service because
    /// [`Service::override_operation`] hands the router a BORROW of one of its own
    /// operations, and it owns the shared engine so the resources it writes live in
    /// the same store the control plane's handlers read.
    fhir_rest: fhir_rest::FhirRestDataPlane<Eng>,
    /// Serves `POST /validate-source`, which is stateless.
    validate_source: validate_source::ValidateSource,
    /// Serves the data-transformation endpoint's REST tagging surface — `GET`/`POST
    /// /tags/{ResourceArn}` and `POST /untag/{ResourceArn}`. Held on the service for
    /// the reason [`fhir_rest`] is, and owns the shared engine so the tags it writes
    /// are the ones the awsJson tagging operations read.
    rest_tagging: rest_tagging::RestTagging<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            fhir_rest: fhir_rest::FhirRestDataPlane::new(Arc::clone(&engine)),
            validate_source: validate_source::ValidateSource::new(),
            rest_tagging: rest_tagging::RestTagging::new(Arc::clone(&engine)),
            entry: crate::ServiceDefinition::new(engine),
            input_validation: input_validation_gates::<Eng>(),
        }
    }
}

/// One [`ModeledInputValidation`] gate per modelled operation this crate serves.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation this crate serves (one per file in `src/ops/`), because the rule
    // is the service's and not one operation's: an operation whose input declares no
    // constraint simply never has a violation to state, which the gate reads off that
    // operation's own input schema. The one modelled operation with no module —
    // `UpdateProfileWithAgent`, declined by the scaffold generator and listed in
    // `src/ops/SKIPPED_STUBS.md` — has no input shape to validate against here and is
    // answered by the framework's unroutable-operation path, exactly as before.
    gate!(
        gates;
        create_data_transformation_profile::CreateDataTransformationProfile,
        create_fhir_datastore::CreateFHIRDatastore,
        delete_data_transformation_profile::DeleteDataTransformationProfile,
        delete_fhir_datastore::DeleteFHIRDatastore,
        describe_data_transformation_job::DescribeDataTransformationJob,
        describe_fhir_datastore::DescribeFHIRDatastore,
        describe_fhir_export_job::DescribeFHIRExportJob,
        describe_fhir_import_job::DescribeFHIRImportJob,
        get_data_transformation_profile::GetDataTransformationProfile,
        list_data_transformation_jobs::ListDataTransformationJobs,
        list_data_transformation_profile_versions::ListDataTransformationProfileVersions,
        list_data_transformation_profiles::ListDataTransformationProfiles,
        list_fhir_datastores::ListFHIRDatastores,
        list_fhir_export_jobs::ListFHIRExportJobs,
        list_fhir_import_jobs::ListFHIRImportJobs,
        list_tags_for_resource::ListTagsForResource,
        publish_data_transformation_profile::PublishDataTransformationProfile,
        restore_fhir_datastore::RestoreFHIRDatastore,
        start_data_transformation_job::StartDataTransformationJob,
        start_fhir_export_job::StartFHIRExportJob,
        start_fhir_import_job::StartFHIRImportJob,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_data_transformation_profile::UpdateDataTransformationProfile,
        update_fhir_datastore::UpdateFHIRDatastore,
    );
    gates
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// guards stays the single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModeledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
                return refused;
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a rule the model declares: this service's
/// modelled `ValidationException`, with the constraint-violation sentence in its
/// `Message` member and in the framework's lowercase `message` beside it.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation: an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violations = reported_violations(model_violations::<Op>(protocol, req)?);
    // The sentence AWS itself returns for a constraint failure, built from the member
    // path, the offending value and the violated constraint by the framework's own
    // renderer — the same text the generic 400 carried, now in the modelled member.
    let error = ConstraintRefusal::new(refusal_message(&violations));
    protocol
        .serialize_error(
            &error,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            crate::scaffolded::wire_http_status(VALIDATION_EXCEPTION_MODELED_STATUS),
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this gate
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "the modelled ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// Every rule of the model `Op`'s input breaks, or `None` when it breaks none.
///
/// Both refusal classes the framework describes structurally are collected: a required
/// member absent (found while reading the request) and a value outside a declared
/// constraint (found while walking the value that was read).
fn model_violations<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Vec<Violation>> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => {
                    let violations = validate(&input, Op::INPUT_SCHEMA);
                    if violations.is_empty() {
                        return None;
                    }
                    ProtocolError::Invalid(violations)
                }
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error)? {
        Refusal::Invalid(violations) => Some(violations),
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not a constraint
        // violation in any case.
        _ => None,
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Three HealthLake surfaces are not awsJson1_0 RPC calls: a data store's FHIR
        // REST endpoint, and on the data-transformation endpoint the
        // `/validate-source` check and the REST tagging surface (`/tags` and
        // `/untag`). Each is addressed by path and HTTP method and carries no
        // `X-Amz-Target`, so the protocol's routing key cannot name them and the
        // framework would otherwise refuse them at the codec with `400
        // SerializationException`. They are routed FIRST, because this seam is the one
        // place a service can name a request the model's routing key cannot.
        //
        // A request that DOES name an RPC target is an RPC call whatever its path, so
        // it is left to the modelled path below: these surfaces are non-RPC by
        // construction.
        if req.headers().get("x-amz-target").is_none() {
            let path = non_rpc::request_path(req.uri());
            if crate::state::parse_datastore_fhir_path(path).is_some() {
                return Some(&self.fhir_rest);
            }
            if req.method() == "POST" && non_rpc::addresses_validate_source(path) {
                return Some(&self.validate_source);
            }
            if rest_tagging::addressed_tagging(req.method(), req.uri()).is_some() {
                return Some(&self.rest_tagging);
            }
        }
        // Otherwise, put this service's input-validation gate in front of the modelled
        // operation the request names, so a value outside a constraint the model
        // declares — or a required member left out — is refused with the MODELLED
        // `ValidationException`, whose `Message` member carries the constraint
        // envelope, instead of the framework's generic 400 body. The gate then hands
        // every request the model accepts to the generated operation, which the routing
        // table still owns; a request that names no modelled operation resolves as it
        // always did.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        //
        // A constraint refusal of an operation this crate SERVES is answered before
        // this seam is reached, from the modelled `ValidationException` shape
        // ([`ModeledInputValidation`]) — a body this seam cannot choose. What reaches
        // here is a refusal of a request that named no such operation, and it is
        // worded with the same envelope so the two cannot diverge.
        match refusal {
            // Constraint violations are stated the way this service states them
            // — code and message together; see `refused_for`.
            Refusal::Invalid(violations) => Some(refused_for(violations)),
            // Authentication is the framework's answer to give; this service
            // words it no differently.
            _ => None,
        }
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

#[cfg(test)]
mod tests {
    //! Every operation this crate serves has an input-validation gate.
    //!
    //! The gate set is written out by name in `input_validation_gates` — the operation
    //! TYPE is what names the input shape and its schema, and the erased routing table
    //! cannot hand that back — so this is what keeps the two in step: an operation added
    //! to the model (and so to `src/ops/`) without a gate would quietly go back to
    //! answering a constraint refusal in the framework's generic envelope, and this test
    //! fails naming it.

    /// This service's Smithy model, as it ships beside this crate.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-healthlake-healthlake.json"
    );

    /// This service's own Smithy namespace. The shipped model also carries shapes from
    /// `com.amazon.awshealthlakedatatransformationfrontendservice`, which declares
    /// operations this service does not bind (and even a second `ListTagsForResource`),
    /// so the census is taken over the namespace the service shape itself lives in —
    /// `Issue-Shape-Id`/`smithy_query(query="list_operations")`:
    /// `com.amazonaws.healthlake#HealthLake`.
    const SERVICE_NAMESPACE: &str = "com.amazonaws.healthlake#";

    /// The operations the scaffold generator declined to emit a module for, read from
    /// the census it writes rather than from a second hand list here.
    const SKIPPED_STUBS: &str = include_str!("ops/SKIPPED_STUBS.md");

    #[test]
    fn every_served_operation_has_an_input_validation_gate() {
        let model = std::fs::read_to_string(MODEL_PATH).expect("this crate ships its Smithy model");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the Smithy model is JSON");
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(id, _)| id.starts_with(SERVICE_NAMESPACE))
            .filter(|(_, shape)| shape.get("type").is_some_and(|kind| kind == "operation"))
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "the model declares no operations, so this test proves nothing"
        );
        let skipped: Vec<&str> = SKIPPED_STUBS
            .lines()
            .filter_map(|line| line.strip_prefix("// SKIPPED: "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        for operation in &operations {
            if skipped.contains(operation) {
                assert!(
                    !gates.contains_key(*operation),
                    "operation {operation} has no module, so it cannot have a gate"
                );
                continue;
            }
            assert!(
                gates.contains_key(*operation),
                "operation {operation} has no input-validation gate, so a request that \
                 violates its model would be refused without the modelled Message"
            );
        }
        assert_eq!(
            gates.len(),
            operations.len() - skipped.len(),
            "the gate set must cover every served operation, got {} gates for {} modelled \
             operations of which {} are skipped",
            gates.len(),
            operations.len(),
            skipped.len()
        );
    }
}
