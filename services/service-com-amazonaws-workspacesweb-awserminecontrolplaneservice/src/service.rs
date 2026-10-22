// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-workspacesweb-awserminecontrolplaneservice` service: how it claims requests and how it words the
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
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;

/// Every operation whose modelled `@http` URI pattern puts LITERAL segments AFTER a
/// greedy `{label+}`, each paired with the routing key the generated table holds it under.
///
/// Nothing about the ROUTE is written down here: the method, the literal segments and
/// their positions are all read off the operation's own input schema (`@http`), so this
/// list cannot drift from the model - a renamed or re-routed operation changes the schema
/// constant, not a string here. The SET is the whole class this model declares
/// (`smithy_query(query="digest")`): the sixteen `Associate*`/`Disassociate*` operations
/// on a portal's sub-resources, `ListIdentityProviders`, and the two trust-store
/// certificate reads. Every other pattern in the model either ends at its greedy label or
/// uses non-greedy labels (`/portals/{portalId}/sessions/{sessionId}`), so the defect
/// below does not reach them.
///
/// A function rather than a `static`, because a `static` initializer may not read other
/// `static`s.
fn suffixed_greedy_inputs() -> [(&'static Schema, &'static str); 19] {
    use crate::ops::types as t;
    [
        (
            t::ASSOCIATE_BROWSER_SETTINGS_INPUT_SCHEMA,
            "AssociateBrowserSettings",
        ),
        (
            t::ASSOCIATE_DATA_PROTECTION_SETTINGS_INPUT_SCHEMA,
            "AssociateDataProtectionSettings",
        ),
        (
            t::ASSOCIATE_IP_ACCESS_SETTINGS_INPUT_SCHEMA,
            "AssociateIpAccessSettings",
        ),
        (
            t::ASSOCIATE_NETWORK_SETTINGS_INPUT_SCHEMA,
            "AssociateNetworkSettings",
        ),
        (
            t::ASSOCIATE_SESSION_LOGGER_INPUT_SCHEMA,
            "AssociateSessionLogger",
        ),
        (t::ASSOCIATE_TRUST_STORE_INPUT_SCHEMA, "AssociateTrustStore"),
        (
            t::ASSOCIATE_USER_ACCESS_LOGGING_SETTINGS_INPUT_SCHEMA,
            "AssociateUserAccessLoggingSettings",
        ),
        (
            t::ASSOCIATE_USER_SETTINGS_INPUT_SCHEMA,
            "AssociateUserSettings",
        ),
        (
            t::DISASSOCIATE_BROWSER_SETTINGS_INPUT_SCHEMA,
            "DisassociateBrowserSettings",
        ),
        (
            t::DISASSOCIATE_DATA_PROTECTION_SETTINGS_INPUT_SCHEMA,
            "DisassociateDataProtectionSettings",
        ),
        (
            t::DISASSOCIATE_IP_ACCESS_SETTINGS_INPUT_SCHEMA,
            "DisassociateIpAccessSettings",
        ),
        (
            t::DISASSOCIATE_NETWORK_SETTINGS_INPUT_SCHEMA,
            "DisassociateNetworkSettings",
        ),
        (
            t::DISASSOCIATE_SESSION_LOGGER_INPUT_SCHEMA,
            "DisassociateSessionLogger",
        ),
        (
            t::DISASSOCIATE_TRUST_STORE_INPUT_SCHEMA,
            "DisassociateTrustStore",
        ),
        (
            t::DISASSOCIATE_USER_ACCESS_LOGGING_SETTINGS_INPUT_SCHEMA,
            "DisassociateUserAccessLoggingSettings",
        ),
        (
            t::DISASSOCIATE_USER_SETTINGS_INPUT_SCHEMA,
            "DisassociateUserSettings",
        ),
        (
            t::LIST_IDENTITY_PROVIDERS_INPUT_SCHEMA,
            "ListIdentityProviders",
        ),
        (
            t::GET_TRUST_STORE_CERTIFICATE_INPUT_SCHEMA,
            "GetTrustStoreCertificate",
        ),
        (
            t::LIST_TRUST_STORE_CERTIFICATES_INPUT_SCHEMA,
            "ListTrustStoreCertificates",
        ),
    ]
}

/// The path component of a request URI, parsed with `http::Uri` rather than by splitting
/// the string: a query value may itself contain `://` or `/`, which a hand-rolled split
/// would mistake for the authority or for a path segment.
fn request_path(uri: &str) -> Option<String> {
    Some(
        uri.parse::<http::Uri>()
            .ok()?
            .path_and_query()?
            .path()
            .to_string(),
    )
}

/// A path's segments, with the leading and trailing `/` dropped. A pattern's query
/// literals (everything after `?`) do not take part in path matching.
fn trimmed_segments(path: &str) -> Vec<&str> {
    let trimmed = path.split('?').next().unwrap_or(path).trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed.split('/').collect()
}

/// Whether a pattern segment is a label (`{Name}` or `{Name+}`), i.e. a caller value
/// rather than a literal the pattern declares.
fn is_label(segment: &str) -> bool {
    segment.starts_with('{')
}

/// Whether a pattern segment is a GREEDY label (`{Name+}`), which the protocol lets bind
/// more than one path segment.
fn is_greedy_label(segment: &str) -> bool {
    segment.starts_with('{') && segment.ends_with("+}")
}

/// How many LITERAL segments a modelled `@http` URI pattern declares after its greedy
/// label, or `None` when it has no greedy label, nothing follows it, or what follows is
/// not all literal.
///
/// The literal requirement is what makes removing those segments safe: a literal is
/// declared by the pattern, so it carries no information the operation needs. A pattern
/// that put another LABEL after the greedy one would be carrying a caller value there,
/// and is left to the framework rather than cut.
fn trailing_segment_count(pattern: &str) -> Option<usize> {
    let segments = trimmed_segments(pattern);
    let greedy = segments.iter().position(|segment| is_greedy_label(segment))?;
    let trailing = &segments[greedy + 1..];
    (!trailing.is_empty() && trailing.iter().all(|segment| !is_label(segment)))
        .then_some(trailing.len())
}

/// Whether `path` matches `pattern`, where `pattern`'s greedy label is followed by
/// `trailing` further segments: each segment before the label must match literally (or be
/// a one-segment label), the label must take at least one segment, and the pattern's
/// trailing segments must be the path's last ones.
fn matches_suffixed_pattern(pattern: &str, path: &str, trailing: usize) -> bool {
    let pattern_segments = trimmed_segments(pattern);
    let path_segments = trimmed_segments(path);
    // The label needs at least one segment of its own on top of the fixed ones.
    if path_segments.len() < pattern_segments.len() || pattern_segments.len() <= trailing {
        return false;
    }
    let leading = pattern_segments.len() - trailing - 1;
    let leads_match = pattern_segments[..leading]
        .iter()
        .zip(&path_segments[..leading])
        .all(|(pattern, actual)| is_label(pattern) || pattern == actual);
    let tail_match = pattern_segments[pattern_segments.len() - trailing..]
        == path_segments[path_segments.len() - trailing..];
    leads_match && tail_match
}

/// `path` with its last `trailing` segments removed, rooted and without a trailing slash,
/// or `None` when it does not have more than that many segments.
fn path_without_trailing_segments(path: &str, trailing: usize) -> Option<String> {
    let segments = trimmed_segments(path);
    if segments.len() <= trailing {
        return None;
    }
    Some(format!("/{}", segments[..segments.len() - trailing].join("/")))
}

/// The routing key of the operation this service serves on `method` + `uri` through a
/// pattern with literal segments after a greedy label, and how many trailing segments
/// that pattern declares.
///
/// Nothing about the route is written here: each candidate's method and URI pattern are
/// read off that operation's own input schema, and only the segment arithmetic above is
/// this file's. The rule is deliberately kept LOCAL rather than hoisted into a shared
/// crate: it exists only because the vendored router mis-resolves these patterns, which
/// the framework gap report (`issues/.../greedy-label-swallows-trailing-literal`) asks
/// to fix — at which point this whole block is DELETED rather than shared. Two other
/// service crates carry their own copy of the same arithmetic
/// (`grep -rln 'fn trimmed_segments' services/*/src/service.rs`), and they go away the
/// same way.
fn suffixed_greedy_route(method: &str, uri: &str) -> Option<(&'static str, usize)> {
    let path = request_path(uri)?;
    suffixed_greedy_inputs()
        .into_iter()
        .find_map(|(schema, operation)| {
            let route = schema.http()?;
            if route.method() != method {
                return None;
            }
            let trailing = trailing_segment_count(route.uri())?;
            matches_suffixed_pattern(route.uri(), &path, trailing).then_some((operation, trailing))
        })
}

/// The same request with the `trailing` literal sub-resource segments removed, so the
/// greedy `@httpLabel` binds the label value alone.
///
/// The cut is made on the same SEGMENTS the match was made on, so a request target that
/// differs only in a trailing slash is adapted the same way rather than leaving a stray
/// segment in the label.
///
/// `None` means the request could not be adapted - its target did not parse, or its body
/// is not the buffered bytes every dispatched request carries - which the caller reports
/// exactly as the framework's own "named no modelled operation" answer rather than
/// serving the operation a request it never received.
fn without_trailing_segments(req: &RtRequest, trailing: usize) -> Option<RtRequest> {
    let parsed = req.uri().parse::<http::Uri>().ok()?;
    let path_and_query = parsed.path_and_query()?;
    let kept = path_without_trailing_segments(path_and_query.path(), trailing)?;
    let rewritten = match path_and_query.query() {
        None => kept,
        Some(query) => format!("{kept}?{query}"),
    };

    let body = req.body().bytes()?.to_vec();
    let mut out = RtRequest::new(aws_smithy_types::body::SdkBody::from(body));
    out.set_uri(rewritten.as_str()).ok()?;
    out.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: a repeated or multi-valued header must keep every
        // value, exactly as the router's own request rebuild does.
        out.headers_mut().append(name.to_string(), value.to_string());
    }
    Some(out)
}

/// Serves the operations whose modelled URI pattern ends in literal segments after a
/// greedy `@httpLabel`.
///
/// The framework resolves such a pattern by letting the greedy label consume EVERY
/// remaining path segment. Two things then go wrong for
/// `PUT /portals/{portalArn+}/userSettings`: the route lookup never matches it (the
/// trailing `userSettings` literal has no segment left to match) and falls through to
/// `PUT /portals/{portalArn+}`, and the label would bind `portalArn` to the ARN plus
/// `/userSettings`, a value the modelled `ARN` pattern rejects. This adapter is what
/// [`EmulatedService::override_operation`] hands such a request to: it names the
/// operation from the model's own routes and runs it against the request with the
/// trailing literal segments - declared by the pattern, so carrying no information -
/// removed. Deserialization, validation and the handler itself are unchanged.
///
/// Only the DISPATCHED request is rewritten; the [`OperationContext`] still carries the
/// request parts the client sent, so a handler reading the URI off `ctx` would see the
/// original target.
///
/// The defect is filed as
/// `issues/.../20261022T060000Z-gap-smithy-schema-server-greedy-label-swallows-trailing-literal.md`;
/// this adapter goes away when the vendored router is fixed.
#[derive(Debug, Default)]
struct GreedyLabelSuffixRoute;

impl ErasedOperation for GreedyLabelSuffixRoute {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let routed = suffixed_greedy_route(req.method(), req.uri()).and_then(
                |(operation, trailing)| {
                    let op = svc.definition().routing_table().get(operation)?;
                    Some((op, without_trailing_segments(req, trailing)?))
                },
            );
            match routed {
                Some((op, adjusted)) => op.run(svc, &adjusted, ctx).await,
                // Nothing this service models answers the request after all; report it
                // exactly as the framework would have.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The `com-amazonaws-workspacesweb-awserminecontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    greedy_label_suffix_route: GreedyLabelSuffixRoute,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            greedy_label_suffix_route: GreedyLabelSuffixRoute,
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
        // A request whose modelled URI pattern puts literal segments after a greedy
        // `@httpLabel` is served through [`GreedyLabelSuffixRoute`]: the generated
        // routing table cannot name it (it falls through to the pattern without the
        // suffix) and the greedy label would swallow that suffix. Every other request
        // resolves through the generated routing table.
        suffixed_greedy_route(req.method(), req.uri())
            .map(|_| &self.greedy_label_suffix_route as &dyn ErasedOperation)
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
        let _ = refusal;
        None
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
    //! The segment rules the greedy-label adapter decides on, over the shapes a modelled
    //! `@http` pattern takes: a greedy label at the end, a greedy label with literals
    //! after it, a greedy label with another label after it, and no label at all. Also
    //! that every operation this crate's model declares with a literal-suffixed greedy
    //! pattern really is in the candidate list, so the list cannot silently fall behind
    //! the model.

    use super::*;

    #[test]
    fn a_path_is_split_into_its_segments_however_it_is_written() {
        // (case, path, segments)
        let rows: [(&str, &str, &[&str]); 7] = [
            ("rooted", "/portals", &["portals"]),
            ("trailing slash", "/portals/", &["portals"]),
            ("two segments", "/portals/abc", &["portals", "abc"]),
            ("unrooted", "portals/abc", &["portals", "abc"]),
            ("root", "/", &[]),
            ("empty", "", &[]),
            (
                "pattern query literals are not path segments",
                "/portals/{portalArn+}/userSettings?userSettingsArn",
                &["portals", "{portalArn+}", "userSettings"],
            ),
        ];

        for (case, path, expected) in rows {
            let actual = trimmed_segments(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn only_a_greedy_label_followed_by_literals_declares_a_trailing_count() {
        // (case, pattern, trailing count)
        let rows: [(&str, &str, Option<usize>); 7] = [
            ("greedy label at the end", "/portals/{portalArn+}", None),
            ("no label at all", "/portals", None),
            (
                "non-greedy labels",
                "/portals/{portalId}/sessions/{sessionId}",
                None,
            ),
            (
                "one literal after the greedy label",
                "/portals/{portalArn+}/userSettings",
                Some(1),
            ),
            (
                "two literals after the greedy label",
                "/trustStores/{trustStoreArn+}/certificates/list",
                Some(2),
            ),
            (
                "a label after the greedy label carries a value, so it is not cut",
                "/portals/{portalArn+}/userSettings/{name}",
                None,
            ),
            (
                "query literals do not count",
                "/portals/{portalArn+}/userSettings?userSettingsArn",
                Some(1),
            ),
        ];

        for (case, pattern, expected) in rows {
            let actual = trailing_segment_count(pattern);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_suffixed_pattern_matches_exactly_the_paths_it_declares() {
        const PATTERN: &str = "/portals/{portalArn+}/userSettings";
        let trailing = trailing_segment_count(PATTERN).expect("the pattern is literal-suffixed");

        // (case, path, matches)
        let rows: [(&str, &str, bool); 8] = [
            ("one label segment", "/portals/abc/userSettings", true),
            (
                "a greedy label spanning several segments",
                "/portals/arn:aws:workspaces-web:us-east-1:1:portal/abc/userSettings",
                true,
            ),
            ("trailing slash", "/portals/abc/userSettings/", true),
            ("the suffix missing", "/portals/abc", false),
            ("the label missing", "/portals/userSettings", false),
            (
                "another leading literal",
                "/trustStores/abc/userSettings",
                false,
            ),
            ("another suffix", "/portals/abc/browserSettings", false),
            (
                "the suffix not last",
                "/portals/abc/userSettings/extra",
                false,
            ),
        ];

        for (case, path, expected) in rows {
            let actual = matches_suffixed_pattern(PATTERN, path, trailing);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected matches == {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn cutting_the_trailing_segments_leaves_the_label_value_alone() {
        // (case, path, trailing, cut path)
        let rows: [(&str, &str, usize, Option<&str>); 5] = [
            (
                "one segment cut",
                "/portals/abc/userSettings",
                1,
                Some("/portals/abc"),
            ),
            (
                "a trailing slash is cut the same way",
                "/portals/abc/userSettings/",
                1,
                Some("/portals/abc"),
            ),
            (
                "a multi-segment label value is kept whole",
                "/portals/arn:aws:x:1:portal/abc/userSettings",
                1,
                Some("/portals/arn:aws:x:1:portal/abc"),
            ),
            (
                "two segments cut",
                "/trustStores/abc/certificates/list",
                2,
                Some("/trustStores/abc"),
            ),
            ("nothing left to cut", "/userSettings", 1, None),
        ];

        for (case, path, trailing, expected) in rows {
            let actual = path_without_trailing_segments(path, trailing);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_request_path_is_read_with_a_uri_parser_not_a_split() {
        // (case, target, path)
        let rows: [(&str, &str, Option<&str>); 4] = [
            (
                "origin form",
                "/portals/abc/userSettings",
                Some("/portals/abc/userSettings"),
            ),
            (
                "absolute form",
                "https://workspaces-web.us-east-1.amazonaws.com/portals/abc",
                Some("/portals/abc"),
            ),
            (
                "a query value carrying a scheme is not the authority",
                "/portals/abc/userSettings?userSettingsArn=https://x/y",
                Some("/portals/abc/userSettings"),
            ),
            ("not a uri", "http://[", None),
        ];

        for (case, target, expected) in rows {
            let actual = request_path(target);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The candidate list is hand-written but it must not fall behind the model: every
    /// pattern in it has to be literal-suffixed (an operation the vendored router CAN
    /// resolve must not be diverted through the adapter), and its route must come off the
    /// operation's own schema. There is no generated list of every input schema in this
    /// crate and `OperationRoutingTable` exposes no iteration, so completeness of the SET
    /// is what the framework gap report asks to make unnecessary; what can be asserted
    /// here is that nothing in the list is diverted wrongly, and the wire suite covers
    /// each listed operation end to end.
    #[test]
    fn every_candidate_pattern_is_literal_suffixed_and_read_off_its_schema() {
        for (schema, operation) in suffixed_greedy_inputs() {
            let route = schema
                .http()
                .unwrap_or_else(|| panic!("case {operation:?}: the schema must bind a route"));
            assert!(
                trailing_segment_count(route.uri()).is_some(),
                "case {operation:?}: {} is not literal-suffixed, so it does not belong in \
                 the candidate list",
                route.uri()
            );
            assert!(
                !route.method().is_empty(),
                "case {operation:?}: the method must come off the schema"
            );
        }
    }
}
