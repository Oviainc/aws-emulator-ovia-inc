// SPDX-License-Identifier: CC0-1.0

//! The request-claim rule for the FOUR emulated services whose SigV4 signing name is
//! `lex`, stated once here and read by every one of them.
//!
//! `aws.auth#sigv4` names `lex` on all four Lex-family models, while each one serves its
//! own endpoint (`smithy_query(query="service_info")` on each model gives the endpoint
//! prefixes `models-v2-lex`, `runtime-v2-lex`, `models.lex` and `runtime.lex` against
//! that ONE signing name, and all four are `restJson1`). Sharing a signing name across a
//! service family and its API versions is normal AWS and is not an error condition: AWS
//! separates the four by endpoint HOST, which an emulator cannot read, since a client
//! configured with one local `endpoint_url` sends the same `Host` for all four.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! all four see the key `lex`: a service that claims on the signing name ALONE claims
//! every sibling's traffic too, the router finds four claimants and — refusing to serve a
//! request by an arbitrary one — answers `500 AmbiguousService`, a code no AWS model
//! carries, for every Lex request.
//!
//! The claim is therefore [`crate::claims_request`]'s rule — the signing name AND the
//! request's own modelled wire surface, which for these four REST models is the first URI
//! path segment — narrowed below wherever MORE THAN ONE of them publishes that segment.
//! What separates them there is the modelled route (method + URI pattern), which is what
//! distinguishes `restJson1` APIs on the wire. Source for every segment and route named
//! here: `smithy_query(query="digest")` on each of the four models, and each service
//! crate's own `OPERATIONS.md` route lines for the 107-operation V2 control plane.

use smithy_server_runtime::ErasedServiceDefinition;

/// The Smithy shape id of each emulated service that signs as `lex`, as
/// `definition().identity().shape_id()` reports it. A member names itself by its shape id
/// rather than by a flag the call site passes, so the ownership tables below cannot
/// disagree with the service reading them.
///
/// The `models-v2-lex` control plane (Lex V2 model building: bots, locales, intents,
/// slots, slot types, versions, exports, imports, test sets).
pub const MODELS_V2: &str = "com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2";
/// The `runtime-v2-lex` data plane (`RecognizeText`, `RecognizeUtterance`, sessions).
pub const RUNTIME_V2: &str = "com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0";
/// The `models.lex` control plane — the 2017-04-19 V1 model-building API.
pub const MODELS_V1: &str =
    "com.amazonaws.lexmodelbuildingservice#AWSDeepSenseModelBuildingService";
/// The `runtime.lex` data plane — the 2016-11-28 V1 runtime API, bound under `/bot/…`.
pub const RUNTIME_V1: &str = "com.amazonaws.lexruntimeservice#AWSDeepSenseRunTimeService";

/// How a first path segment that more than one Lex-family model publishes is split
/// between its sharers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shared {
    /// The sharers bind DIFFERENT routes under this segment, so the whole route — method
    /// plus URI pattern — tells them apart: each one claims only a request its own model
    /// binds a route for ([`crate::addresses_modeled_route`]).
    ///
    /// A path under such a segment that NO model binds therefore reaches nobody, and the
    /// router answers it `404 UnknownService` rather than one service's protocol routing
    /// error. That is the narrow cost of not guessing: no AWS operation lives on such a
    /// path, while handing the whole segment to one sharer would make every one of the
    /// other's operations in it unreachable.
    ByRoute,
    /// The sharers bind the SAME routes under this segment, so no part of the request
    /// tells them apart (AWS tells them apart by endpoint host, which is not on the wire
    /// here). The named service answers the whole segment; its sharer's operations there
    /// are unreachable, which keeps the owner's working instead of refusing both with
    /// `500 AmbiguousService`.
    OwnedBy(&'static str),
}

/// Every first URI path segment more than one Lex-family model publishes, and how it is
/// split. Each sharer reads this one table, so the claims cannot drift into a path with
/// two claimants (the `AmbiguousService` refusal) or none.
///
/// The segments each model publishes: `bots`, `exports`, `imports`, `policy`, `tags`,
/// `builtins`, `createuploadurl`, `testsets`, `testexecutions`, `testsetdiscrepancy` and
/// `testsetgenerations` (V2 control plane); `bots`, `intents`, `slottypes`, `builtins`,
/// `exports`, `imports`, `migrations` and `tags` (V1 control plane); `bots` (V2 runtime);
/// `bot` (V1 runtime). So FIVE segments are shared:
///
/// * `bots` — all three of the V2 control plane, the V1 control plane and the V2 runtime
///   bind it, and almost entirely with different routes: the V2 control plane's literals
///   are `botversions`/`botlocales`/`botaliases`, the V2 runtime's are
///   `botAliases`/`botLocales`/`sessions` (segment matching is case-SENSITIVE, as HTTP
///   path semantics and the framework's route matcher both are), and the V1 control
///   plane's are `versions`/`aliases`/`channels`/`utterances`. So the route decides —
///   except for the one route shape two of them bind identically, in
///   [`IDENTICAL_ROUTES`].
/// * `builtins` — the V1 control plane binds `GET /builtins/intents`,
///   `GET /builtins/intents/{signature}` and `GET /builtins/slottypes`; the V2 control
///   plane binds `POST /builtins/locales/{localeId}/intents` and
///   `POST /builtins/locales/{localeId}/slottypes`. Disjoint, so the route decides.
/// * `exports` — the V1 control plane binds only `GET /exports`; the V2 control plane
///   binds `PUT`/`POST /exports` and `GET`/`PUT`/`DELETE /exports/{exportId}`. Disjoint,
///   so the route decides.
/// * `imports` — the V1 control plane binds `POST /imports` (StartImport) and
///   `GET /imports/{importId}` (GetImport), and the V2 control plane binds BOTH of those
///   same two routes (ListImports and DescribeImport) plus `PUT /imports` and
///   `DELETE /imports/{importId}`. Every V1 route here is therefore identical to a V2
///   one, so the route separates nothing and an owner must decide; the V2 control plane
///   owns it, being the member that also binds the two routes nobody else does — and the
///   V1 model marks its whole API `@deprecated` ("Amazon Lex V1 is deprecated. Use Amazon
///   Lex V2 instead.", `smithy_query(query="operation",operation="StartImport")`).
/// * `tags` — the two control planes bind the identical three routes (`GET`, `POST` and
///   `DELETE /tags/{resourceArn}`) and nothing else under the segment, so again only an
///   owner can decide, and the V2 control plane owns it. The cost is that the V1
///   tagging operations are unreachable, the same cost the Bedrock family's `tags` split
///   accepts (see [`crate::bedrock`]); all three of them, and both V1 import operations,
///   are scaffolded 501s today, so nothing implemented is lost either way.
const SHARED_SEGMENTS: &[(&str, Shared)] = &[
    ("bots", Shared::ByRoute),
    ("builtins", Shared::ByRoute),
    ("exports", Shared::ByRoute),
    ("imports", Shared::OwnedBy(MODELS_V2)),
    ("tags", Shared::OwnedBy(MODELS_V2)),
];

/// One route shape two Lex-family models bind IDENTICALLY inside a [`Shared::ByRoute`]
/// segment, and how the claim is split when the route itself — the discriminator that
/// segment relies on — cannot decide.
///
/// The split is the LABEL's own modelled shape: the two models constrain the identifier
/// in that path position differently, and an identifier only one of them admits names a
/// resource only that one can own. Where both admit it, or neither does, the `wider`
/// member answers, so such a request always has exactly one claimant.
struct IdenticalRoute {
    /// The HTTP method, compared exactly (as the framework's route matcher compares it).
    method: &'static str,
    /// The path pattern's segments, with [`LABEL`] standing for an `@httpLabel`.
    pattern: &'static [&'static str],
    /// Which pattern segment (0-based) carries the deciding identifier.
    label_index: usize,
    /// Claims the route when [`IdenticalRoute::admits_narrow`] accepts that label.
    narrower: &'static str,
    /// Claims it otherwise.
    wider: &'static str,
    /// Whether the label is one the `narrower` member's modelled label shape admits.
    admits_narrow: fn(&str) -> bool,
}

/// Every route shape two Lex-family models bind identically, consulted BEFORE
/// [`SHARED_SEGMENTS`] because for such a request the route is exactly what does not
/// discriminate.
///
/// One route shape qualifies today: `DELETE /bots/{…}` is `DeleteBot` in BOTH control
/// planes, and both are implemented here, so neither may be made unreachable. Their
/// labels are different shapes, which is the one signal left on the wire: V2's `botId` is
/// `@length(min: 10, max: 10)` with `@pattern(^[0-9a-zA-Z]+$)` — exactly what its own
/// `CreateBot` mints — while V1's `name` is a bot NAME, `@length(min: 2, max: 50)` with
/// `@pattern(^([A-Za-z]_?)+$)`, which admits no digit and no ten-character bound
/// (`smithy_query(query="operation",operation="DeleteBot")` on each model). So a label
/// shaped like a V2 id reaches V2, and every other label reaches V1.
///
/// The residual overlap — a V1 bot name of exactly ten letters, which both shapes admit —
/// reaches V2, where it is answered with V2's own modelled error. Nothing in the request
/// can settle that case: AWS settles it by endpoint host, which a client configured with
/// one local endpoint does not vary, and the V1 model marks its whole API `@deprecated`
/// in favour of V2.
const IDENTICAL_ROUTES: &[IdenticalRoute] = &[IdenticalRoute {
    method: "DELETE",
    pattern: &["bots", LABEL],
    label_index: 1,
    narrower: MODELS_V2,
    wider: MODELS_V1,
    admits_narrow: is_models_v2_id,
}];

/// Whether `label` is an identifier the V2 control plane's `Id` shape admits:
/// `@length(min: 10, max: 10)` and `@pattern(^[0-9a-zA-Z]+$)`
/// (`smithy_query(query="shape",shape="Id")` on the V2 model). Written as the two bounds
/// it is rather than as a regex; the shape cited above is its source.
fn is_models_v2_id(label: &str) -> bool {
    label.len() == 10 && label.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Stands for an `@httpLabel` segment in [`IDENTICAL_ROUTES`]: it matches any one
/// non-empty segment. Not a wildcard over several segments — a route shape's segment
/// count is matched exactly, as the framework's own route matcher does for a pattern with
/// no greedy label.
const LABEL: &str = "{}";

/// Whether the Lex-family service whose generated `ServiceDefinition` is `definition`
/// claims `parts`.
///
/// * `path_segments` — that service's generated `crate::ROUTE_PATH_SEGMENTS`, the first
///   URI path segments its model declares.
///
/// All four models are `restJson1` (`smithy_query(query="service_info")`), so there is no
/// RPC form to claim by name and the fourth argument of [`crate::claims_request`] is
/// always `None` here. The signing name is read off `definition`, so a member cannot
/// claim under a name its model does not declare.
pub fn claims_request(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
    path_segments: &[&str],
) -> bool {
    let identity = definition.identity();
    if !crate::claims_request(parts, identity.signing_name(), path_segments, None) {
        return false;
    }
    if let Some(claimant) = identical_route_claimant(parts.method.as_str(), parts.uri.path()) {
        return identity.shape_id() == claimant;
    }
    match shared_split(parts.uri.path()) {
        None => true,
        Some(Shared::ByRoute) => crate::addresses_modeled_route(parts, definition),
        Some(Shared::OwnedBy(owner)) => identity.shape_id() == owner,
    }
}

/// How the first segment of `path` is split, or `None` when only one Lex-family model
/// publishes it (the common case, where the first segment already names exactly one
/// member). Matching is exact segment equality, as the framework's own discriminator is.
fn shared_split(path: &str) -> Option<Shared> {
    let segment = super::first_path_segment(path)?;
    SHARED_SEGMENTS
        .iter()
        .find(|(shared, _)| *shared == segment)
        .map(|(_, split)| *split)
}

/// The ONE member that claims `method` + `path` when two models bind that route shape
/// identically, or `None` when no entry of [`IDENTICAL_ROUTES`] matches it.
///
/// The segment count must match exactly and each pattern segment must equal the request's,
/// except [`LABEL`], which matches any one segment. Which member claims it is then decided
/// by that label's modelled shape — see [`IdenticalRoute`].
fn identical_route_claimant(method: &str, path: &str) -> Option<&'static str> {
    let segments = path_segments(path);
    let route = IDENTICAL_ROUTES.iter().find(|route| {
        route.method == method
            && route.pattern.len() == segments.len()
            && route
                .pattern
                .iter()
                .zip(&segments)
                .all(|(expected, actual)| *expected == LABEL || expected == actual)
    })?;
    let label = segments.get(route.label_index).copied().unwrap_or_default();
    Some(if (route.admits_narrow)(label) {
        route.narrower
    } else {
        route.wider
    })
}

/// The `/`-separated, non-empty segments of a URI path, as the framework's route matcher
/// splits them: leading and trailing `/` are ignored, so `/bots/B/` and `/bots/B` are the
/// same two segments.
fn path_segments(path: &str) -> Vec<&str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    }
}

#[cfg(test)]
mod tests {
    //! The parts of the rule that need no built service: which segments are shared, how
    //! each one is split, and which route shapes are owned outright.

    use super::*;

    /// Every service that signs as `lex`, for the tables above to be checked against.
    const MEMBERS: &[&str] = &[MODELS_V2, RUNTIME_V2, MODELS_V1, RUNTIME_V1];

    /// Every segment in the table is split exactly one way, and a segment only one model
    /// publishes is not in it at all.
    #[test]
    fn the_shared_segments_are_declared_once_each() {
        // (case, path, expected split)
        let rows: [(&str, &str, Option<Shared>); 14] = [
            ("the V2 control plane's bots", "/bots", Some(Shared::ByRoute)),
            (
                "a deep V2 control-plane path under it",
                "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/slottypes",
                Some(Shared::ByRoute),
            ),
            (
                "a V2 runtime path under it",
                "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1/text",
                Some(Shared::ByRoute),
            ),
            (
                "a V1 control-plane path under it",
                "/bots/OrderFlowers/versions/$LATEST",
                Some(Shared::ByRoute),
            ),
            ("builtins", "/builtins/intents", Some(Shared::ByRoute)),
            ("exports", "/exports/EXPORT1234", Some(Shared::ByRoute)),
            (
                "the two control planes' identical import routes",
                "/imports",
                Some(Shared::OwnedBy(MODELS_V2)),
            ),
            (
                "and the deeper one of the pair",
                "/imports/IMPORT1234",
                Some(Shared::OwnedBy(MODELS_V2)),
            ),
            (
                "the two control planes' identical tagging routes",
                "/tags/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
                Some(Shared::OwnedBy(MODELS_V2)),
            ),
            // Segments exactly one of the four models publishes: not shared, so the
            // first segment alone already names one claimant.
            ("the V1 runtime's own space", "/bot/Pizza/alias/PROD", None),
            ("the V1 control plane's slot types", "/slottypes", None),
            ("the V2 control plane's policies", "/policy/arn", None),
            ("a near miss on a shared segment", "/botsextra", None),
            ("the root path", "/", None),
        ];

        for (case, path, expected) in rows {
            let actual = shared_split(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: on the one route shape two models bind identically, the claim follows the
    /// label's own modelled shape — a V2 `Id` (ten `[0-9a-zA-Z]`) reaches the V2 control
    /// plane and every other label the V1 one — and the shape is matched on its method and
    /// its whole segment count, so nothing deeper or shallower is drawn into it.
    #[test]
    fn an_identical_route_shape_is_split_by_the_shape_of_its_label() {
        // (case, method, path, expected claimant)
        //
        // The label rows span what the two models' label shapes differ along: length (10
        // against anything else), the character classes each admits (the V1 name admits
        // `_` and no digit; the V2 id admits digits and no `_`), and the case of the
        // letters, which neither shape constrains.
        let rows: [(&str, &str, &str, Option<&str>); 14] = [
            (
                "a minted V2 botId: ten alphanumerics",
                "DELETE",
                "/bots/BOT1234567",
                Some(MODELS_V2),
            ),
            (
                "a V2 botId of ten digits",
                "DELETE",
                "/bots/0000000000",
                Some(MODELS_V2),
            ),
            (
                "a V2 botId of ten lower-case letters",
                "DELETE",
                "/bots/zzzzzzzzzz",
                Some(MODELS_V2),
            ),
            (
                "the same route with a trailing slash",
                "DELETE",
                "/bots/A1B2C3D4E5/",
                Some(MODELS_V2),
            ),
            (
                "a V1 bot name shorter than ten characters",
                "DELETE",
                "/bots/DoomedBot",
                Some(MODELS_V1),
            ),
            (
                "a V1 bot name longer than ten characters",
                "DELETE",
                "/bots/NeverExistedBot",
                Some(MODELS_V1),
            ),
            (
                // Ten characters, but `_` is outside the V2 id's pattern and inside the
                // V1 name's.
                "a ten-character V1 bot name carrying an underscore",
                "DELETE",
                "/bots/Order_Flow",
                Some(MODELS_V1),
            ),
            (
                "the shortest V1 bot name its length bound allows",
                "DELETE",
                "/bots/Ab",
                Some(MODELS_V1),
            ),
            (
                // Thirteen digits: outside the V2 id's length and outside the V1 name's
                // pattern, so NEITHER shape admits it and the wider member answers, which
                // refuses it with its own modelled error rather than nobody claiming it.
                "a label neither shape admits",
                "DELETE",
                "/bots/0123456789012",
                Some(MODELS_V1),
            ),
            // Another method on the same path shape is bound by ONE model, so the route
            // already decides and no label split is needed.
            (
                "the describe on the same shape",
                "GET",
                "/bots/BOT1234567",
                None,
            ),
            ("the create above it", "PUT", "/bots", None),
            // A deeper delete is a different route shape, and its literals differ between
            // the models.
            (
                "a deeper delete",
                "DELETE",
                "/bots/BOT1234567/botversions/DRAFT",
                None,
            ),
            ("the segment alone", "DELETE", "/bots", None),
            (
                "another segment's delete",
                "DELETE",
                "/exports/EXPORT1234",
                None,
            ),
        ];

        for (case, method, path, expected) in rows {
            let actual = identical_route_claimant(method, path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {method} {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every owner named in either table is a member of this family, and no segment or
    /// route shape is declared twice.
    #[test]
    fn every_owner_is_a_member_and_nothing_repeats() {
        let mut seen: Vec<&str> = Vec::new();
        for (segment, split) in SHARED_SEGMENTS {
            assert!(
                !seen.contains(segment),
                "segment {segment:?} is declared more than once"
            );
            seen.push(segment);
            if let Shared::OwnedBy(owner) = split {
                assert!(
                    MEMBERS.contains(owner),
                    "segment {segment:?} is owned by {owner:?}, which is not a member of \
                     the lex family"
                );
            }
        }

        let mut routes: Vec<(&str, &[&str])> = Vec::new();
        for route in IDENTICAL_ROUTES {
            let key = (route.method, route.pattern);
            assert!(
                !routes.contains(&key),
                "route {} {:?} is declared more than once",
                route.method,
                route.pattern
            );
            routes.push(key);
            for claimant in [route.narrower, route.wider] {
                assert!(
                    MEMBERS.contains(&claimant),
                    "route {} {:?} is claimed by {claimant:?}, which is not a member of \
                     the lex family",
                    route.method,
                    route.pattern
                );
            }
            assert_ne!(
                route.narrower, route.wider,
                "route {} {:?} splits between one member and itself, so the label shape \
                 decides nothing",
                route.method,
                route.pattern
            );
            assert!(
                route.pattern.get(route.label_index) == Some(&LABEL),
                "route {} {:?} splits on segment {}, which is not a label",
                route.method,
                route.pattern,
                route.label_index
            );
            // An identical route shape only ever lives inside a segment the family
            // shares: elsewhere the first segment already names one claimant.
            let segment = route.pattern.first().copied().unwrap_or_default();
            assert!(
                SHARED_SEGMENTS.iter().any(|(shared, _)| *shared == segment),
                "route {} {:?} is split under {segment:?}, which no two members share",
                route.method,
                route.pattern
            );
        }
    }
}
