// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrock-amazonbedrockcontrolplaneservice` service: how it claims requests and how it words the
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
//! - `override_refusal` — EDITED HERE. An ARN this service checks itself is refused with
//!   this service's own sentence (see "This service's own ARN check" below), and a guardrail
//!   policy carrying more entries than its quota admits with the quota's own sentence
//!   ([`refused_policy_entry_quota`]); every other request refused for breaking a modelled
//!   input constraint is worded by the ONE formatter in [`validation`]: a `@pattern` is
//!   quoted as the expression the whole value must match, the value a member held is quoted
//!   for the shapes and bindings this service is recorded quoting it for, and a violation on a
//!   collection's ELEMENT is reported at the collection member with that element shape's
//!   whole constraint set. See the inline note on that method.
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
//! # This service's own ARN check
//!
//! Amazon Bedrock reads an ARN a caller submits ITSELF, ahead of the constraints the model
//! declares for the member carrying it, and answers one of its OWN short sentences for an
//! ARN it will not accept — never the `N validation error(s) detected: Value '…' at '…'
//! failed to satisfy constraint: Member must satisfy regular expression pattern: …`
//! envelope the framework words a constraint violation in. Every sentence and the catalog
//! of resources two of them cover ([`crate::state::SELF_VALIDATED_RESOURCE_TYPES`]) live
//! in [`crate::state`]; this file is where the two layers that produce them sit:
//!
//! * [`Gated`], the wrapper every modelled operation is served behind
//!   (`override_operation`), refuses an ARN of this service's own namespace that names
//!   ANOTHER region before the modelled walk runs. It is the layer that can: the walk's
//!   refusal reaches `override_refusal` with the violations ALONE — no request — so the
//!   region the request addresses is not knowable there. That same wrapper is where IAM's
//!   authorization decision is taken for every operation ([`authorization`]).
//! * `override_refusal` words what is left, from the violation's own member expression and
//!   value ([`refused_arn`]): an ARN of this service's namespace that is in-region but is
//!   not a well-formed ARN of the resource type it names, and a value that is no ARN at
//!   all submitted to a member whose modelled form admits nothing but the ARN of one of
//!   those resources ([`pattern_requires_an_arn`]).
//!
//! A value submitted to a member the model lets carry a bare NAME beside an ARN keeps the
//! framework's wording: such a caller named a resource rather than spelling an ARN, so
//! there is no ARN for this service to read. So does an ARN of a resource type this
//! service does not validate itself, which is what the corpus records for them.
//!
//! # The authorization layer every handler runs under
//!
//! AWS decides whether the CALLER may perform the action at the IAM layer, in front of the
//! service: a principal whose identity-based policies do not grant `bedrock:<Operation>` —
//! or that an explicit `Deny` covers — is refused the modelled `AccessDeniedException`
//! (403) and the call performs nothing. That is a property of the request's identity and
//! the action it names rather than of what any one handler does, so it is decided ONCE,
//! here in [`Gated`], for every operation of this service ([`authorization`] holds the
//! decision, the one operation whose model declares no such refusal, and the refusal's
//! wording). The resource half is [`RequestArns::named_resource`] beside
//! [`authorization::judged_resource`]: the ARN the request names of the family its URI
//! space addresses — or, where its URI carries that one resource's own ARN resource id
//! instead of the whole ARN, the ARN built from that id — else that family's collection, so
//! a policy scoped to one resource or to one family is evaluated against what the caller
//! addressed and never against an ARN of another family the request happens to carry.

pub mod authorization;
pub mod data_automation_tagging;
pub mod validation;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{path, PatternTrait};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};

/// The error code this service answers a refused ARN with: the modelled
/// `ValidationException`, which every operation the corpus records this refusal for
/// declares at `@httpError(400)` (`smithy_query(query="operation",…)`). It is also the
/// framework's own default code for a refusal, restated here because `override_refusal`
/// and the refusal channel replace the code and the message TOGETHER.
///
/// `CreateModelCopyJob` is the one operation that does NOT list it, yet the corpus
/// records Amazon Bedrock answering 400 `ValidationException` there too, so the code is
/// the same for every member: it is reported through the framework's refusal channel
/// rather than through an operation's error enum, which is what lets that one answer it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The input members whose ARN names a resource in ANOTHER region BY DESIGN, so
/// [`Gated`]'s region check does not apply to them, keyed by the operation that
/// declares the member and the member's own path.
///
/// Both members today carry the SOURCE model of a copy. `CreateModelCopyJob` "Copies a
/// model to another region so that it can be used there"
/// (`smithy_query(query="doc",shape="CreateModelCopyJob")`) and its input declares no
/// target-region member
/// (`smithy_query(query="operation",operation="CreateModelCopyJob")`: `sourceModelArn`,
/// `targetModelName`, `modelKmsKeyId`, `targetModelTags`, `clientRequestToken`), so the
/// region the request addresses IS the target and the source model it names lives in
/// another one. Refusing that ARN for naming another region would refuse the whole
/// operation — and `ListModelCopyJobs$sourceModelArnEquals` "Filters for model copy jobs
/// in which the Amazon Resource Name (ARN) of the source model is equal to the value that
/// you specify", so it carries that same ARN and would make the filter unusable.
const CROSS_REGION_ARN_INPUTS: &[(&str, &str)] = &[
    ("CreateModelCopyJob", "sourceModelArn"),
    ("ListModelCopyJobs", "sourceModelArnEquals"),
];

/// The `com-amazonaws-bedrock-amazonbedrockcontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`Gated`] per modelled operation, keyed by the routing key the model
    /// names that operation by ([`Operation::NAME`], the same key the generated routing
    /// table uses). Built once here, never per request; each entry carries the operation's
    /// TYPE and a handle on the one shared engine, so this is a table of names rather than
    /// a second set of operations.
    gated: HashMap<&'static str, Box<dyn ErasedOperation>>,
    /// The Bedrock Data Automation half of the three tagging paths this service binds:
    /// built once here over the same shared engine, and handed back from
    /// `override_operation` for a tagging request whose `resourceARN` names one of that
    /// service's resources (see [`data_automation_tagging`]).
    data_automation_tagging: data_automation_tagging::DataAutomationTagging<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            gated: gated_table::<Eng>(&engine),
            data_automation_tagging: data_automation_tagging::DataAutomationTagging::new(
                Arc::clone(&engine),
            ),
            entry: crate::ServiceDefinition::new(engine),
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
        // This service signs as `bedrock` — a signing name it SHARES with every other
        // Bedrock-family service (`bedrock-runtime`, `bedrock-agent`,
        // `bedrock-agent-runtime`, `bedrock-data-automation` and
        // `bedrock-data-automation-runtime` all carry `aws.auth#sigv4` name `bedrock`
        // while serving their own endpoint prefix). The router resolves a request's
        // routing key from the SigV4 credential scope first, so all six see the key
        // `bedrock`: claiming on the signing name ALONE makes each of them a claimant for
        // every Bedrock request, and the router — which refuses to pick one — answers
        // `500 AmbiguousService`, a code no AWS model carries, for ALL of it, leaving the
        // whole family unreachable.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the one
        // rule `service_common::bedrock` states for the whole family (which narrows
        // `service_common::claims_request` with the family's shared-segment split): the
        // signing name AND a first URI path segment this `restJson1` model declares
        // (`guardrails`, `foundation-models`, `logging`, `custom-models`,
        // `model-customization-jobs`, …), matched by EXACT segment equality. `None` for
        // the RPC argument: every operation of this model is bound to a URI, so it names
        // no service out of band.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve every modelled operation through its [`Gated`] wrapper, so the two
        // decisions AWS takes in front of a handler are taken there: an ARN of this
        // service's own namespace naming another region is refused with this service's own
        // sentence BEFORE the modelled constraint walk words a refusal of its own, and the
        // caller's authorization for the action is decided before the operation reads or
        // writes anything. The wrapper delegates every other request to the generated
        // operation in the routing table, which stays the only thing that serves it.
        //
        // Keyed on the routing key the model names each operation by, so a request that
        // names no modelled operation resolves exactly as it did before.
        let key = self.entry.protocol().routing_key(req).ok()?;
        // The three tagging paths this service binds are bound identically by
        // `bedrock-data-automation`, and the family's claim rule gives the whole segment to
        // this service — so a tagging request whose `resourceARN` names one of THAT
        // service's resources is served out of that model here, which is the first seam
        // handed the body the ARN lives in (see [`data_automation_tagging`]). Every other
        // request, tagging or not, is this service's own.
        if self.data_automation_tagging.serves(key, req) {
            return Some(&self.data_automation_tagging);
        }
        self.gated.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Amazon Bedrock reports an ARN of its own namespace that it will not accept with
        // a short sentence of its own, not with the clause the framework writes for a
        // member outside its `@pattern`: the two are produced by different rules, and a
        // client that branches on the message shape takes the wrong branch when a
        // constraint envelope arrives where the service's own sentence belongs.
        //
        // The region half of that check is answered one layer earlier, by [`Gated`]
        // (it needs the request). What reaches here is an ARN this service accepts the
        // REGION of and whose resource is not a well-formed one of the type the ARN
        // itself names, so the sentence is the format one.
        //
        // `find_map`, not "every violation": the ARN check runs ahead of the modelled
        // constraints, so it decides the answer even when the same request also breaks
        // another rule.
        //
        // The second sentence of this service's own is the guardrail policy ENTRY QUOTA
        // ([`refused_policy_entry_quota`]): a policy collection past the maximum the model
        // declares for it is over a service quota, which Amazon Bedrock reports as that
        // quota rather than as a member that is too long.
        //
        // EVERY OTHER input-constraint refusal — a value outside a length, range or enum,
        // a member that is not an ARN at all, an absent required member — is worded by the
        // ONE formatter in [`validation`], which states the AWS envelope with the rules the
        // framework's generic wording does not: a `@pattern` quoted as the expression the
        // WHOLE value must match, the value a member held quoted for the shapes and the
        // bindings this service is recorded quoting it for, and a violation on an ELEMENT of a
        // collection reported against the collection MEMBER carrying that element shape's
        // whole constraint set rather than against an indexed element position. Either way
        // the code is the modelled `ValidationException` (HTTP 400) the framework would
        // also have named: only the words change.
        match refusal {
            Refusal::Invalid(violations) => worded_refusal(violations),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has no wording of
            // its own for keeps the framework's — an authentication refusal, say, which is
            // not an input-constraint failure at all.
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

/// This service's own sentence for `violation`, when the value it refused is one this
/// service reads as an ARN itself.
///
/// Two sentences, both decided from the violation ALONE — the member's declared expression
/// and the value that broke it, which is all this seam is handed:
///
/// * a well-formed ARN of a resource this service validates itself
///   ([`crate::state::SELF_VALIDATED_RESOURCE_TYPES`]) whose resource is not a well-formed
///   one of the type the ARN itself names reports that form
///   ([`crate::state::invalid_arn_format_message`]);
/// * a value that is no ARN at all, submitted to a member whose modelled form admits
///   nothing but an ARN ([`pattern_requires_an_arn`]) of a resource this service validates
///   itself ([`crate::state::pattern_names_self_validated_resource`]), reports that the ARN
///   cannot be read ([`crate::state::ARN_INVALID`]). The MEMBER decides this, not the
///   value: a member that also admits a bare name carries no ARN to read when a name
///   arrives, a member declared as the ARN of a resource outside that catalog keeps the
///   refusal the model's own constraints word — which is what the corpus records for an
///   `evaluation-job` identifier — and a value that IS an ARN but of a resource type
///   outside it keeps that refusal too.
///
/// Only a `@pattern` violation: a value outside a LENGTH bound is refused for its size
/// rather than for its ARN form, and an absent member carries no value to read an ARN off
/// at all. The declared expression comes with the violation, which is how this seam —
/// handed the violations alone — applies the same "the member admits an ARN" test
/// [`admits_an_arn`] makes against the input schema: a caller who puts an ARN into a
/// member that is not one (a name, a description, a tag value) breaks that member's own
/// rule, not an ARN rule, and the corpus records the constraint envelope for it.
fn refused_arn(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(declared) = &violation.kind else {
        return None;
    };
    if !pattern_admits_an_arn(declared) {
        return None;
    }
    let value = violation.value.as_deref()?;
    if let Some(message) = crate::state::invalid_arn_format_message(value) {
        return Some(message);
    }
    let unreadable = pattern_requires_an_arn(declared)
        && crate::state::pattern_names_self_validated_resource(declared)
        && ::service_common::arn::parse(value).is_none();
    unreadable.then(|| crate::state::ARN_INVALID.to_string())
}

/// This service's own sentence for `violation`, when what it refused is a guardrail policy
/// carrying more entries than its quota admits.
///
/// Amazon Bedrock bounds the entries of a guardrail policy by a service QUOTA and reports
/// going past one as that quota — the fixed sentence
/// [`crate::state::GUARDRAIL_POLICY_ENTRY_QUOTAS`] holds for the member — rather than
/// through the member-length clause [`validation`] words a modelled `@length` with: the two
/// say different things to a caller, which is "you sent too many entries" against "one entry
/// is malformed", and only the first names a quota to ask for an increase of.
///
/// Decided from the violation ALONE, which is all this seam is handed: the member path the
/// walk reports the collection at, and the `@length` MAXIMUM as the class that failed. Only
/// the maximum — a collection under its minimum is a malformed request rather than too much
/// of one, so it keeps the clause the model's own constraint words.
fn refused_policy_entry_quota(violation: &Violation) -> Option<String> {
    let ViolationKind::MaxLength(_) = &violation.kind else {
        return None;
    };
    crate::state::guardrail_policy_entry_quota_message(&violation.path).map(str::to_string)
}

/// One modelled operation, served behind the two decisions AWS takes in front of a
/// handler: this service's own ARN check, and IAM's authorization of the action.
///
/// It carries the operation TYPE — the input shape to read the request into and the input
/// `Schema` to read each member's declared form off — and a handle on the ONE shared
/// engine, which is what the authorization decision reads the caller's identity and its
/// policies through ([`authorization`]). Every request it does not refuse is delegated to
/// the one operation instance the generated routing table built from that same engine, so
/// there is no second instance of any operation and no second store.
struct Gated<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> Gated<Op, Eng> {
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

/// The refusal this service words for `violations`: its own ARN sentence first, then the
/// guardrail entry quota, then the one formatter in [`validation`] for every other
/// input-constraint failure.
///
/// Reached from `override_refusal` AND from [`Gated`], which answers the same refusal one
/// layer earlier for the request whose value the validating walk withheld — so both states
/// one refusal in one voice.
fn worded_refusal(violations: &[Violation]) -> Option<Refused> {
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: violations
            .iter()
            .find_map(refused_arn)
            .or_else(|| violations.iter().find_map(refused_policy_entry_quota))
            .or_else(|| validation::envelope(violations))?,
    })
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Gated<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let region = crate::state::request_region(ctx.parts());
            // The protocol is read, used and dropped inside this one statement, and the
            // operation to delegate to is resolved into a value further down:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across the
            // await below would make this future non-`Send`. Both ARNs the two gates read
            // off the request come from ONE deserialization of its input.
            let read = request_arns::<Op>(svc.definition().protocol(), req, &region);
            if let Some((path, value)) = read.cross_region {
                return rejection::<Op>(svc, req, path, value);
            }
            // IAM's decision, taken before the operation reads or writes anything: the
            // action is this operation's own name under this service's prefix, and the
            // resource is the one the REQUEST addresses. A refused request never reaches
            // the delegate, so it mints nothing, moves no status and reports nothing.
            let denial = if authorization::layer_judges(Op::NAME) {
                let resource =
                    authorization::judged_resource(ctx, read.named_resource, req.uri());
                authorization::authorize(&*self.engine, ctx, Op::NAME, &resource).err()
            } else {
                None
            };
            if let Some(denial) = denial {
                return authorization::refusal_response(svc.definition().protocol(), &denial);
            }
            // The modelled input constraints, worded by this service's own seam
            // ([`Service::override_refusal`], the ONE formatter) over the violations as a
            // reading of the REQUEST restores them: the validating walk withholds the value
            // of a `@sensitive` member as it records the violation, and that seam is handed
            // the violations alone, so a clause of this service that quotes the value has
            // to be worded where the request is still in hand. A refusal the seam declines
            // is left to the walk behind the delegate exactly as before, and the violations
            // are the framework's own, so nothing is refused here that it would accept.
            let refused =
                ::service_common::validation::refusing_violations::<Op>(
                    svc.definition().protocol(),
                    req,
                );
            if !refused.is_empty() {
                if let Some(stated) = svc.override_refusal(&Refusal::Invalid(refused.clone())) {
                    return svc.definition().refusal_response(
                        ProtocolError::Invalid(refused),
                        Some(stated),
                        req,
                    );
                }
            }
            let delegate = svc.definition().routing_table().get(Op::NAME);
            match delegate {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed (or not) as
        // the MODEL says for the operation it names, so authentication is decided by the
        // modelled operation this wrapper stands in front of.
        None
    }
}

/// What the two gates in front of a handler read off the request's own input, in ONE
/// deserialization of it.
#[derive(Debug, Default)]
struct RequestArns {
    /// The first ARN-bearing member whose value is an ARN of a resource this service
    /// validates itself ([`crate::state::SELF_VALIDATED_RESOURCE_TYPES`]) naming a region
    /// other than the one the request addresses, as its member path and the ARN it
    /// carried.
    ///
    /// Only a member the MODEL declares an ARN-admitting `@pattern` for is read here, so a
    /// description or a tag value that happens to contain an ARN is not one of these.
    cross_region: Option<(String, String)>,
    /// The ARN of one of this service's own resources the request CARRIES at a member the
    /// model declares as an ARN member ([`carries_an_arn`]): an identifier member spells it
    /// (`GetCustomModel$modelIdentifier` as a `custom-model` ARN,
    /// `DeleteResourcePolicy$resourceArn` in the URI, `ListTagsForResource$resourceARN` in
    /// the body). Whether it is the resource the action is judged against is
    /// [`authorization::judged_resource`]'s question, because several operations carry an
    /// ARN of ANOTHER family beside their target — a job create names the model it runs —
    /// and only an ARN of the family the request's URI space addresses is the resource it
    /// addressed.
    ///
    /// Read from the VALUE rather than from the member's declared form, because a member the
    /// model lets carry a bare name beside an ARN addresses the resource itself when a
    /// caller spells the ARN.
    named_resource: Option<String>,
}

/// Read both ARNs off the operation's own input.
///
/// The request is deserialized against `Op`'s input schema (so a member is placed by the
/// model, not by where it happens to sit in the wire form) and every string the built input
/// carries is collected by [`service_common::validation::submitted_leaves`], the one walk in
/// the tree that spells a member path the way the framework's constraint walk does. Only
/// TOP-LEVEL members, because a member schema carries its target's members nowhere, so a
/// nested member's declared form is not reachable from the input schema; no ARN-bearing
/// member of the operations these rules are recorded for is nested. Nothing is read out of
/// the STORE, which is what keeps both decisions independent of whether the resource exists.
///
/// Neither ARN is reported for a body no codec can read, which is a failure the framework's
/// own walk words.
///
/// KNOWN BOUND: the leaf walk stops at the budget its module states
/// (`MAX_RECORDED_LEAVES` = 256 leaves, `MAX_RECORDED_BYTES` = 64 KiB), so a request
/// whose earlier strings exhaust it — a 200-entry tag list ahead of the ARN member, say —
/// can leave a later top-level ARN uninspected. The miss is FAIL-SAFE in both readings:
/// such a request falls back to the framework's constraint envelope (the divergence the
/// region check removes) rather than being refused for an ARN it carries, and it is
/// authorized against the collection of the family it addressed rather than against an ARN
/// this layer did not see — never against a resource the caller did not name. Closing it
/// needs the shared walk to offer an unbounded form (or the framework to hand a built input
/// to an operation), which is a change to shared code this issue does not own.
fn request_arns<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    region: &str,
) -> RequestArns {
    let Ok(mut deserializer) = protocol.deserialize_input(req, Op::INPUT_SCHEMA) else {
        return RequestArns::default();
    };
    let read = <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    let Ok(input) = read else {
        return RequestArns::default();
    };
    let leaves = service_common::validation::submitted_leaves(&input, Op::INPUT_SCHEMA);
    RequestArns {
        cross_region: leaves
            .iter()
            .find(|(member_path, value)| {
                admits_an_arn(Op::INPUT_SCHEMA, member_path)
                    && !CROSS_REGION_ARN_INPUTS.contains(&(Op::NAME, member_path.as_str()))
                    && crate::state::arn_names_another_region(value, region)
            })
            .cloned(),
        named_resource: leaves
            .iter()
            .find(|(member_path, value)| {
                carries_an_arn(Op::INPUT_SCHEMA, member_path)
                    && authorization::names_a_resource_of_this_service(value)
            })
            .map(|(_, value)| value.clone()),
    }
}

/// Whether `input_schema` declares the TOP-LEVEL member at `member_path` as one that
/// carries an ARN.
///
/// The member decides this, not the value: a free-text member a caller happens to put an
/// ARN into — a description, a tag value, a job name — does not address that resource, and
/// a NESTED value is not a member of the input at all (a job's `inferenceConfig` names the
/// model it runs, which is not what the action acts on). The MODEL answers it two ways, and
/// either suffices: the member's `@pattern` admits an ARN ([`admits_an_arn`]), or the model
/// NAMES the member as one — `DeleteResourcePolicy$resourceArn` carries the documented "The
/// Amazon Resource Name (ARN) of the resource" under a shape that declares a `@length` and
/// no `@pattern` at all
/// (`smithy_query(query="operation",operation="DeleteResourcePolicy")`).
fn carries_an_arn(input_schema: &Schema, member_path: &str) -> bool {
    let declared_member = input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_path)
    });
    declared_member
        && (admits_an_arn(input_schema, member_path)
            || member_path.to_ascii_lowercase().ends_with("arn"))
}

/// Whether `input_schema` declares the top-level member at `member_path` with a
/// `@pattern` that admits an ARN.
///
/// Read off the member's own `@pattern` trait, so the MODEL decides which members carry
/// an ARN ([`pattern_admits_an_arn`]).
fn admits_an_arn(input_schema: &Schema, member_path: &str) -> bool {
    declared_pattern(input_schema, member_path).is_some_and(pattern_admits_an_arn)
}

/// Whether a declared `@pattern` admits an ARN.
///
/// Every ARN-valued member of this service declares an anchored expression over `arn:…`
/// — on its own (`ModelCopyJobArn`, `CustomModelArn`) or as one alternative beside a bare
/// name (`ProvisionedModelId`, `ModelInvocationJobIdentifier`), which is why ADMITTING
/// one is the test rather than requiring one. A member whose expression mentions no ARN
/// at all (a name, a description, a tag value) carries no ARN for this service to read,
/// whatever a caller puts in it.
fn pattern_admits_an_arn(pattern: &str) -> bool {
    pattern.contains("arn:")
}

/// Whether a declared `@pattern` admits NOTHING BUT an ARN — the test for a member this
/// service reads as an ARN itself, whatever arrived in it.
///
/// Decided over the expression's TOP-LEVEL alternatives ([`alternatives`]): the member
/// requires an ARN when every one of them does, and an alternative requires one when it
/// begins with the literal `arn:` or is a single group whose own alternatives all require
/// one ([`sole_group`], which refuses a group a quantifier makes optional or repeatable).
/// `ModelArn` and `CustomModelArn` are such members; `ProvisionedModelId`
/// (`^((([0-9a-zA-Z][_-]?)+)|(arn:…))$`) and `ModelInvocationJobIdentifier`
/// (`^((arn:…)?[a-z0-9]{12})$`) are not, because each also admits a bare name.
///
/// A member that spells its ARN alternatives in a form this test cannot read reports
/// NOTHING BUT an ARN and is read as admitting other values, which keeps the framework's
/// constraint envelope for it: the miss is fail-safe in that one direction, so no value is
/// refused for an ARN it does not carry.
fn pattern_requires_an_arn(pattern: &str) -> bool {
    let body = pattern.trim_start_matches('^').trim_end_matches('$');
    !body.is_empty() && alternatives(body).into_iter().all(alternative_requires_an_arn)
}

/// Whether ONE alternative of an expression admits nothing but an ARN.
fn alternative_requires_an_arn(alternative: &str) -> bool {
    let alternative = alternative.trim_start_matches('^');
    alternative.starts_with("arn:")
        || sole_group(alternative).is_some_and(pattern_requires_an_arn)
}

/// The top-level alternatives of a regular expression: the spans `|` separates OUTSIDE
/// every group and character class, with a backslash escaping the character after it.
///
/// One span \u2014 the expression itself \u2014 when it carries no such `|`.
fn alternatives(expression: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut in_class = false;
    let mut escaped = false;
    for (index, character) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => depth = depth.saturating_sub(1),
            '|' if !in_class && depth == 0 => {
                spans.push(&expression[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    spans.push(&expression[start..]);
    spans
}

/// What a regular expression that is ONE group contains, with the group's own `(` and `)`
/// and any `?:` that opens a non-capturing one removed.
///
/// `None` when the expression does not begin a group, when that group closes before the
/// end of the expression, or when a quantifier follows it: a group spelled `(…)?` or `(…)*`
/// admits the EMPTY match beside what it carries, so what it carries does not decide what
/// the expression admits.
fn sole_group(expression: &str) -> Option<&str> {
    if !expression.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_class = false;
    let mut escaped = false;
    for (index, character) in expression.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    // The group has to span the whole expression: anything after it \u2014 a
                    // quantifier or another term \u2014 means the group alone does not decide
                    // what the expression admits.
                    if index + 1 != expression.len() {
                        return None;
                    }
                    let inner = &expression[1..index];
                    return Some(inner.strip_prefix("?:").unwrap_or(inner));
                }
            }
            _ => {}
        }
    }
    None
}

/// The `@pattern` expression `input_schema` declares for its top-level member
/// `member_path`, if it declares one.
fn declared_pattern(input_schema: &Schema, member_path: &str) -> Option<&'static str> {
    let member = input_schema.members().iter().find(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_path)
    })?;
    member
        .traits()?
        .get(&PatternTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<PatternTrait>()
        .map(|pattern| pattern.regex)
}

/// This service's answer to an ARN it refused for naming another region: its own
/// envelope and the status the framework's refusal channel gives a refused input (400),
/// with [`VALIDATION_EXCEPTION`] and
/// [`crate::state::ARN_INVALID_FOR_SERVICE_REGION`] replacing the code and the message
/// together — a client branches on the code, so one without the other is worse than
/// neither.
///
/// The violation carried on the channel names the member and the form the model declares
/// for it, because that is what the channel's structure is; its clause never reaches the
/// wire, since the chosen code and message replace it.
fn rejection<Op: Operation>(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    member_path: String,
    value: String,
) -> aws_smithy_runtime_api::http::Response {
    let declared = declared_pattern(Op::INPUT_SCHEMA, &member_path).unwrap_or_default();
    svc.definition().refusal_response(
        ProtocolError::Invalid(vec![Violation {
            path: member_path,
            kind: ViolationKind::Pattern(declared.to_string()),
            value: Some(value),
            sensitive: false,
        }]),
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: crate::state::ARN_INVALID_FOR_SERVICE_REGION.to_string(),
        }),
        req,
    )
}

/// The [`Gated`] wrapper for EVERY operation this service serves.
///
/// Carrying an ARN is a property of an operation's INPUT SHAPE, and the authorization
/// decision is a property of the request's identity and the action it names, so neither
/// check belongs to a few operations: the wrapper goes in front of all of them —
/// implemented or still scaffolded. A stub answers 501 for a request this service accepts
/// the ARNs of and whose caller IAM permits, and the refusal otherwise, which is the order
/// the real service evaluates them in; leaving an operation out would answer the
/// framework's generic constraint envelope there, or serve an unauthorized caller, the very
/// divergences this removes.
fn gated_table<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut wrappers: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! register {
        ($($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                wrappers.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(Gated::<crate::ops::$module::$operation<Eng>, Eng>::new(
                        Arc::clone(engine),
                    )) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    register!(
        batch_delete_advanced_prompt_optimization_job::BatchDeleteAdvancedPromptOptimizationJob,
        batch_delete_evaluation_job::BatchDeleteEvaluationJob,
        cancel_automated_reasoning_policy_build_workflow::CancelAutomatedReasoningPolicyBuildWorkflow,
        create_advanced_prompt_optimization_job::CreateAdvancedPromptOptimizationJob,
        create_automated_reasoning_policy::CreateAutomatedReasoningPolicy,
        create_automated_reasoning_policy_test_case::CreateAutomatedReasoningPolicyTestCase,
        create_automated_reasoning_policy_version::CreateAutomatedReasoningPolicyVersion,
        create_custom_model::CreateCustomModel,
        create_custom_model_deployment::CreateCustomModelDeployment,
        create_evaluation_job::CreateEvaluationJob,
        create_foundation_model_agreement::CreateFoundationModelAgreement,
        create_guardrail::CreateGuardrail,
        create_guardrail_version::CreateGuardrailVersion,
        create_inference_profile::CreateInferenceProfile,
        create_marketplace_model_endpoint::CreateMarketplaceModelEndpoint,
        create_model_copy_job::CreateModelCopyJob,
        create_model_customization_job::CreateModelCustomizationJob,
        create_model_import_job::CreateModelImportJob,
        create_model_invocation_job::CreateModelInvocationJob,
        create_prompt_router::CreatePromptRouter,
        create_provisioned_model_throughput::CreateProvisionedModelThroughput,
        delete_automated_reasoning_policy::DeleteAutomatedReasoningPolicy,
        delete_automated_reasoning_policy_build_workflow::DeleteAutomatedReasoningPolicyBuildWorkflow,
        delete_automated_reasoning_policy_test_case::DeleteAutomatedReasoningPolicyTestCase,
        delete_custom_model::DeleteCustomModel,
        delete_custom_model_deployment::DeleteCustomModelDeployment,
        delete_enforced_guardrail_configuration::DeleteEnforcedGuardrailConfiguration,
        delete_foundation_model_agreement::DeleteFoundationModelAgreement,
        delete_guardrail::DeleteGuardrail,
        delete_imported_model::DeleteImportedModel,
        delete_inference_profile::DeleteInferenceProfile,
        delete_marketplace_model_endpoint::DeleteMarketplaceModelEndpoint,
        delete_model_invocation_logging_configuration::DeleteModelInvocationLoggingConfiguration,
        delete_prompt_router::DeletePromptRouter,
        delete_provisioned_model_throughput::DeleteProvisionedModelThroughput,
        delete_resource_policy::DeleteResourcePolicy,
        deregister_marketplace_model_endpoint::DeregisterMarketplaceModelEndpoint,
        export_automated_reasoning_policy_version::ExportAutomatedReasoningPolicyVersion,
        get_account_data_retention::GetAccountDataRetention,
        get_advanced_prompt_optimization_job::GetAdvancedPromptOptimizationJob,
        get_automated_reasoning_policy::GetAutomatedReasoningPolicy,
        get_automated_reasoning_policy_annotations::GetAutomatedReasoningPolicyAnnotations,
        get_automated_reasoning_policy_build_workflow::GetAutomatedReasoningPolicyBuildWorkflow,
        get_automated_reasoning_policy_build_workflow_result_assets::GetAutomatedReasoningPolicyBuildWorkflowResultAssets,
        get_automated_reasoning_policy_next_scenario::GetAutomatedReasoningPolicyNextScenario,
        get_automated_reasoning_policy_test_case::GetAutomatedReasoningPolicyTestCase,
        get_automated_reasoning_policy_test_result::GetAutomatedReasoningPolicyTestResult,
        get_custom_model::GetCustomModel,
        get_custom_model_deployment::GetCustomModelDeployment,
        get_evaluation_job::GetEvaluationJob,
        get_foundation_model::GetFoundationModel,
        get_foundation_model_availability::GetFoundationModelAvailability,
        get_guardrail::GetGuardrail,
        get_imported_model::GetImportedModel,
        get_inference_profile::GetInferenceProfile,
        get_marketplace_model_endpoint::GetMarketplaceModelEndpoint,
        get_model_copy_job::GetModelCopyJob,
        get_model_customization_job::GetModelCustomizationJob,
        get_model_import_job::GetModelImportJob,
        get_model_invocation_job::GetModelInvocationJob,
        get_model_invocation_logging_configuration::GetModelInvocationLoggingConfiguration,
        get_prompt_router::GetPromptRouter,
        get_provisioned_model_throughput::GetProvisionedModelThroughput,
        get_resource_policy::GetResourcePolicy,
        get_use_case_for_model_access::GetUseCaseForModelAccess,
        list_advanced_prompt_optimization_jobs::ListAdvancedPromptOptimizationJobs,
        list_automated_reasoning_policies::ListAutomatedReasoningPolicies,
        list_automated_reasoning_policy_build_workflows::ListAutomatedReasoningPolicyBuildWorkflows,
        list_automated_reasoning_policy_test_cases::ListAutomatedReasoningPolicyTestCases,
        list_automated_reasoning_policy_test_results::ListAutomatedReasoningPolicyTestResults,
        list_custom_model_deployments::ListCustomModelDeployments,
        list_custom_models::ListCustomModels,
        list_enforced_guardrails_configuration::ListEnforcedGuardrailsConfiguration,
        list_evaluation_jobs::ListEvaluationJobs,
        list_foundation_model_agreement_offers::ListFoundationModelAgreementOffers,
        list_foundation_models::ListFoundationModels,
        list_guardrails::ListGuardrails,
        list_imported_models::ListImportedModels,
        list_inference_profiles::ListInferenceProfiles,
        list_marketplace_model_endpoints::ListMarketplaceModelEndpoints,
        list_model_copy_jobs::ListModelCopyJobs,
        list_model_customization_jobs::ListModelCustomizationJobs,
        list_model_import_jobs::ListModelImportJobs,
        list_model_invocation_jobs::ListModelInvocationJobs,
        list_prompt_routers::ListPromptRouters,
        list_provisioned_model_throughputs::ListProvisionedModelThroughputs,
        list_tags_for_resource::ListTagsForResource,
        put_account_data_retention::PutAccountDataRetention,
        put_enforced_guardrail_configuration::PutEnforcedGuardrailConfiguration,
        put_model_invocation_logging_configuration::PutModelInvocationLoggingConfiguration,
        put_resource_policy::PutResourcePolicy,
        put_use_case_for_model_access::PutUseCaseForModelAccess,
        register_marketplace_model_endpoint::RegisterMarketplaceModelEndpoint,
        start_automated_reasoning_policy_build_workflow::StartAutomatedReasoningPolicyBuildWorkflow,
        start_automated_reasoning_policy_test_workflow::StartAutomatedReasoningPolicyTestWorkflow,
        stop_advanced_prompt_optimization_job::StopAdvancedPromptOptimizationJob,
        stop_evaluation_job::StopEvaluationJob,
        stop_model_customization_job::StopModelCustomizationJob,
        stop_model_invocation_job::StopModelInvocationJob,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_automated_reasoning_policy::UpdateAutomatedReasoningPolicy,
        update_automated_reasoning_policy_annotations::UpdateAutomatedReasoningPolicyAnnotations,
        update_automated_reasoning_policy_test_case::UpdateAutomatedReasoningPolicyTestCase,
        update_custom_model_deployment::UpdateCustomModelDeployment,
        update_guardrail::UpdateGuardrail,
        update_marketplace_model_endpoint::UpdateMarketplaceModelEndpoint,
        update_provisioned_model_throughput::UpdateProvisionedModelThroughput,
    );
    wrappers
}
