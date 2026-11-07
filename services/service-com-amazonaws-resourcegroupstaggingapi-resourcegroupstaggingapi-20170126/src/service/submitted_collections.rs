// SPDX-License-Identifier: CC0-1.0

//! The one half of a constraint refusal the framework's walk cannot state: the
//! COLLECTION a clause about a collection member is about.
//!
//! A value outside a constraint the model declares is refused before any handler
//! runs, with HTTP 400 `ValidationException` and one clause per violation, each
//! naming the value that was rejected, the member path it sat at and the
//! requirement it failed. The value is the only place the request's own input is
//! reported back, so a client comparing it against what it sent has to see its
//! own input.
//!
//! A `@length` on a map or a list is checked ON THE MEMBER, before the walk
//! descends into the entries, so the only rendering the framework has for the
//! rejected value is a SUMMARY of its size — `<n> items` for a list, `<n>
//! entries` for a map — which is not a value any caller sent. AWS states the
//! collection itself there: a map as `{<key>=<value>, …}`, a list as `[<element>,
//! …]`.
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal)
//! is handed the violations alone and never the request they were found in, so
//! that value cannot be recovered there. [`QuotingSubmittedCollections`] is where
//! it is: a per-operation wrapper, which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the
//! violations and the submitted collections come from the tree's shared
//! collecting walk (`service_common::validation::refusing_violations_with_mapped_echoes`,
//! which runs the protocol's own deserializer and the framework's own
//! `constraints::validate` and owns every bound on what one clause may quote),
//! the restoring rule is the shared
//! `service_common::validation_message::quoting_the_submitted_collection`, each
//! clause is the framework's own `Violation::message`, the envelope around them is
//! the framework's own, and the status and the wire envelope come from the
//! framework's refusal channel. A refusal with NO such clause is left to that
//! channel untouched.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::validation::{refusing_violations_with_mapped_echoes, rendered_map};
use service_common::validation_message::{
    envelope_over, map_entry_count, quoting_the_submitted_collection,
};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refused, Service, Violation,
};

/// The error code a constraint refusal carries, which is the one the framework's
/// own renderer picks for it on this service's JSON protocol: the restated
/// message travels with the code it would have travelled with, because a client
/// branches on the code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One modelled operation of this service, served so that a constraint clause
/// about a collection member states the collection the request carried.
///
/// It makes no routing decision and holds no state of its own: the operation it
/// wraps is the one the generated routing table names for the same key, built
/// from the SAME shared engine, and a request this wrapper does not refuse is
/// handed to it untouched — so the framework still owns the whole deserialize ->
/// validate -> handle -> serialize sequence and every status, shape and message
/// it answers with.
///
/// The cost is DELIBERATE: a request's body is read twice, here and in the
/// operation's own sequence, because the framework hands no already-built input
/// to an operation.
pub struct QuotingSubmittedCollections<Op> {
    operation: Op,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op) -> Self {
        Self { operation }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it
            // is rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across
            // that await would make this future non-`Send`.
            if let Some(refused) = quoting_refusal::<Op>(svc, req) {
                return refused;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must
    /// not change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The refusal `req` earns from `Op`'s own constraints when at least one of its
/// clauses is about a COLLECTION whose submitted value the shared walk could
/// render, or `None` — which is every other request, including one refused only
/// by clauses this changes nothing about, so the framework's own channel answers
/// it exactly as before.
fn quoting_refusal<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let refusing = refusing_violations_with_mapped_echoes::<Op>(svc.definition().protocol(), req).0;
    let mut quoted_a_collection = false;
    let violations: Vec<Violation> = refusing
        .into_iter()
        .map(|(violation, echo)| {
            let stated = quoting_the_submitted_collection(violation.clone(), echo);
            if stated.value == violation.value {
                return violation;
            }
            quoted_a_collection = true;
            as_submitted(&violation, stated, req)
        })
        .collect();
    if !quoted_a_collection {
        return None;
    }
    // Every clause is the framework's own sentence over the violation as stated,
    // and the envelope over them is the framework's own too, so the ONE
    // difference from the refusal that channel would have rendered is the
    // collection a clause about a collection quotes.
    let clauses: Vec<String> = violations.iter().map(Violation::message).collect();
    let message = envelope_over(&clauses)?;
    Some(svc.definition().refusal_response(
        ::aws_smithy_schema_server::ProtocolError::Invalid(violations),
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message,
        }),
        req,
    ))
}

/// `quoted` — the shared rendering of the collection `violation` is about — with a
/// MAP's entries restated in the order the REQUEST carried them rather than in the
/// key order the built input leaves them in.
///
/// A map's entries have an order on the wire and none in the value the codec
/// builds from it (`TagMap` arrives as a `HashMap`), so the shared walk — which
/// reads the built input — can only state one map in key order. The order the
/// caller wrote is still in the request, which this layer has, so it is read from
/// there and only the ORDER changes: the entries, the braces and the
/// `<key>=<value>` form are the shared rendering's ([`rendered_map`]).
///
/// What this layer may NOT do is enlarge the clause: every bound on what one
/// message quotes belongs to the shared walk, so a restated map is taken only when
/// it is exactly as long as the rendering that walk had already decided to carry —
/// the same entries in another order, and therefore the same bytes. A clause that
/// fails any of these keeps `quoted` as the shared walk wrote it: a value that is
/// not a map (a list, whose order the built value does keep), a member below the
/// top level of the body, a body this cannot read, and a member whose entry count
/// differs from the one the walk counted.
fn as_submitted(violation: &Violation, quoted: Violation, req: &RtRequest) -> Violation {
    let (Some(stated), Some(rendered)) = (violation.value.as_deref(), quoted.value.as_deref())
    else {
        return quoted;
    };
    let Some(entries) = map_entry_count(stated) else {
        return quoted;
    };
    if violation.path.contains('.') {
        return quoted;
    }
    let Some(submitted) = submitted_map(req.body().bytes().unwrap_or_default(), &violation.path)
    else {
        return quoted;
    };
    if submitted.len() != entries {
        return quoted;
    }
    let pairs: Vec<(&str, &str)> = submitted
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let in_request_order = rendered_map(&pairs);
    if in_request_order.len() != rendered.len() {
        return quoted;
    }
    Violation {
        value: Some(in_request_order),
        ..quoted
    }
}

/// The entries the request's `member` carried, in the order its JSON object wrote
/// them, or `None` when the body is not an object, carries no such member, or
/// carries one that is not a map of strings.
///
/// The JSON is read by `serde_json`'s own parser, through `serde`'s deserializer
/// seam: nothing here reads the body's bytes itself. The seam is what keeps the
/// ORDER, which a parse into a `Value` would not — that type holds its members in
/// a map of its own.
///
/// It reads the body the server has already buffered and the framework has already
/// deserialized into this operation's input, over the one member a clause is about
/// and skipping every other without building it, so it adds no growth a request can
/// drive beyond that one parse; and what it produces is only used while it is no
/// longer than the rendering the shared walk's own bounds had already admitted (see
/// [`as_submitted`]).
///
/// The member is matched without regard to case because a violation names it in
/// the lowerCamel path the framework states a clause at, while the body carries
/// the wire name the model declares; this service's model declares no two members
/// of one shape that differ only in case.
fn submitted_map(body: &[u8], member: &str) -> Option<Vec<(String, String)>> {
    let mut deserializer = ::serde_json::Deserializer::from_slice(body);
    ::serde::de::DeserializeSeed::deserialize(MemberEntries { member }, &mut deserializer)
        .ok()
        .flatten()
}

/// Reads ONE named member of a JSON object as the string map it holds, keeping the
/// order the document wrote the entries in. Every other member is skipped without
/// being built.
struct MemberEntries<'a> {
    member: &'a str,
}

impl<'de> ::serde::de::DeserializeSeed<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn deserialize<D: ::serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> ::serde::de::Visitor<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(formatter, "a JSON object carrying the member {}", self.member)
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut members: A,
    ) -> Result<Self::Value, A::Error> {
        let mut found: Option<Vec<(String, String)>> = None;
        while let Some(name) = members.next_key::<String>()? {
            if found.is_none() && name.eq_ignore_ascii_case(self.member) {
                found = Some(members.next_value::<OrderedStringMap>()?.0);
            } else {
                members.next_value::<::serde::de::IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// A JSON object of strings as the entries it holds, in document order.
struct OrderedStringMap(Vec<(String, String)>);

impl<'de> ::serde::Deserialize<'de> for OrderedStringMap {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> ::serde::de::Visitor<'de> for Entries {
            type Value = OrderedStringMap;

            fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str("a JSON object of strings")
            }

            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut entries: A,
            ) -> Result<Self::Value, A::Error> {
                let mut held: Vec<(String, String)> = Vec::new();
                while let Some(entry) = entries.next_entry::<String, String>()? {
                    held.push(entry);
                }
                Ok(OrderedStringMap(held))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}
