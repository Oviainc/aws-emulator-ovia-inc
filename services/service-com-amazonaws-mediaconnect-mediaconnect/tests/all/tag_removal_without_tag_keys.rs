// SPDX-License-Identifier: CC0-1.0

//! A tag removal that leaves its `@required` `tagKeys` parameter out, end to end over HTTP,
//! on BOTH removal routes of this service and against subjects that really carry tags.
//!
//! The rule under test: a request whose method and path address a modelled route of this
//! service, and which the REST router matched to no operation only because a `@required`
//! query-bound member of that route is absent, is a refusal of the REQUEST — this service's
//! `BadRequestException` (400) in its `Validation failed:` envelope, naming the absent
//! member — it is never reported as an unimplemented operation, and because it is refused
//! before the handler runs it changes nothing a later caller sees. `tests/all/
//! required_query_parameters.rs` asserts that refusal over every route of this model that
//! binds such a member; this suite adds what a route with no stored subject cannot show:
//! that the refusal leaves the addressed resource's stored tag map exactly as it was, and
//! that the very same route still serves the removal once the parameter is there.
//!
//! The rows span what the rule generalises over. The subjects are every kind of resource the
//! two removals reach — a flow through `DELETE /tags/{ResourceArn}` and each of the three
//! global resources through `DELETE /tags/global/{ResourceArn}` — so both the one-literal and
//! the two-literal path prefix are covered. The queries span the shapes "absent" takes on the
//! wire: no query string at all, an empty one, a query carrying only another parameter, a
//! parameter whose name merely starts with the bound one, and several unrelated parameters —
//! because the router's required-binding check compares parameter NAMES, so a query string
//! carrying no pair named `tagKeys` is absent however much else it carries.
//!
//! What a removal left is read back through the implemented tag reports
//! (`ListTagsForResource` and `ListTagsForGlobalResource`), so a refusal that quietly wrote
//! the store, or a removal that answered its own request, cannot pass.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::collections::BTreeMap;

use serde_json::json;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
// The global tag REPORT, which is how a global subject's stored map is read back.
use crate::global_resource_tag_listing::reported_tags as reported_global_tags;
// The three global-resource creates, the kinds they cover, and the REGIONAL tag write: the
// global tagging suite owns them, so the subjects here are tagged by the same requests.
use crate::global_resource_tags::{create_subject, tag_regional, GlobalKind, EVERY_KIND};
// The clause this service states for an absent required member, and the message reader: the
// required-query suite owns both, so the two suites cannot word the same refusal differently.
use crate::required_query_parameters::{message_of, missing_member_clause};
// The percent-encoder every suite that addresses a resource by an ARN label uses.
use crate::resource_tags::pct;

/// The pairs every subject starts from: more than one key, so a refusal that removed some
/// keys and a removal that removed all of them are both visible, and one key per request the
/// cases below name.
fn starting_tags() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "video".to_string()),
        ("owner".to_string(), "ops".to_string()),
    ])
}

/// The key the serving removal names, and so the only key that stops being carried.
const REMOVED_KEY: &str = "env";

/// The shapes "the request carries no `tagKeys`" takes on the wire, each as `(case, query)`.
///
/// The router's required-binding check reads raw `key[=value]` pairs off the query string and
/// compares their NAMES (`required_binding_score`,
/// `vendor/smithy-schema-server/src/rest_xml.rs`), so every row below supplies no pair named
/// `tagKeys` — whatever else it supplies — and every row must therefore be answered the same
/// way.
const ABSENT_QUERIES: &[(&str, &str)] = &[
    ("no query string at all", ""),
    ("an empty query string", "?"),
    ("a query carrying only another parameter", "?resourceArn=env"),
    (
        "a parameter whose name only starts with the bound one",
        "?tagKeysExtra=env",
    ),
    ("several unrelated parameters", "?first=1&second=2"),
];

/// One resource a tag removal reaches, and the route that reaches it.
enum Subject {
    /// A flow, reached by the REGIONAL removal `DELETE /tags/{ResourceArn}`.
    Flow(String),
    /// A global resource, reached by `DELETE /tags/global/{ResourceArn}`.
    Global(GlobalKind, String),
}

impl Subject {
    /// How a case names this subject: the route it is reached by, and which kind of global
    /// resource it is when that is what the route decides.
    fn label(&self) -> String {
        match self {
            Self::Flow(_) => "a flow / DELETE /tags/{ResourceArn}".to_string(),
            Self::Global(kind, _) => {
                format!("a {kind:?} / DELETE /tags/global/{{ResourceArn}}")
            }
        }
    }

    /// The request target of this subject's removal route, with no query string.
    fn path(&self) -> String {
        match self {
            Self::Flow(arn) => format!("/tags/{}", pct(arn)),
            Self::Global(_, arn) => format!("/tags/global/{}", pct(arn)),
        }
    }

    /// The tag map this subject's own implemented report answers for it, as the store holds
    /// it.
    async fn reported_tags(&self, emulator: &Emulator, case: &str) -> BTreeMap<String, String> {
        match self {
            Self::Flow(arn) => reported_flow_tags(emulator, case, arn).await,
            Self::Global(_, arn) => reported_global_tags(emulator, case, arn).await,
        }
    }
}

/// The tags `ListTagsForResource` reports for a REGIONAL resource, asserted to be a 200
/// carrying a tag map.
///
/// The global twin of this reader is `global_resource_tag_listing::reported_tags`; the
/// regional suite reads the same report through its own harness, so this is the one reader
/// that reaches it from the shared harness.
async fn reported_flow_tags(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, code, body) = emulator
        .send("GET", &format!("/tags/{}", pct(arn)), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing the tags of {arn:?} expected 200, got {status} {code:?}: {body}"
    );
    // An untagged resource has an EMPTY tag map, not an absent member, so an omitted member
    // is a different answer and is not read here as an empty map.
    let reported = body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a tags map, got {body}"));
    reported
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the tag {key:?} must be a string, got {value}")
            });
            (key.clone(), value.to_string())
        })
        .collect()
}

/// Every subject a tag removal reaches, each already carrying [`starting_tags`]: one flow
/// (the regional route) and one of each global resource kind (the global route).
async fn tagged_subjects(emulator: &Emulator) -> Vec<Subject> {
    let requested = json!(starting_tags());

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "tag-removal-subject",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating the fixture flow expected 201, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers a FlowArn, got {created}"))
        .to_string();
    let (status, code, body) = tag_regional(emulator, &flow_arn, requested.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (204, None),
        "tagging the fixture flow expected 204, got {status} {code:?}: {body}"
    );

    let mut subjects = vec![Subject::Flow(flow_arn)];
    for (index, kind) in EVERY_KIND.into_iter().enumerate() {
        let arn = create_subject(
            emulator,
            kind,
            &format!("tag-removal-subject-{index}"),
            Some(requested.clone()),
        )
        .await;
        subjects.push(Subject::Global(kind, arn));
    }
    subjects
}

/// A tag removal that supplies no `tagKeys` is this service's client-input error naming the
/// absent member, on either removal route and whatever else its query string carries; it
/// removes nothing; and the same route serves the removal once the parameter is supplied.
#[tokio::test]
async fn a_tag_removal_without_tag_keys_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let subjects = tagged_subjects(&emulator).await;

    // The member path comes from the framework's own path renderer rather than a second copy
    // of its casing rule, and the sentence from the required-query suite's clause.
    let member_path = aws_smithy_schema_server::constraints::path::member("TagKeys");
    let refusal = mc::validation_failed_message(&missing_member_clause(&member_path));

    for subject in &subjects {
        for (row, query) in ABSENT_QUERIES {
            let case = format!("{} / {row}", subject.label());
            let target = format!("{}{query}", subject.path());
            let (status, code, body) = emulator.send("DELETE", &target, None).await;
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?} (DELETE {target}): a removal leaving its required parameter out \
                 is refused as a client error, got {status} {code:?}: {body}"
            );
            let message = message_of(&body);
            assert_eq!(
                message, refusal,
                "case {case:?}: the refusal names {member_path:?} as absent in this service's \
                 envelope, got {message:?}"
            );

            // Refused before the handler ran, so the stored map is untouched.
            let reported = subject.reported_tags(&emulator, &case).await;
            assert_eq!(
                reported,
                starting_tags(),
                "case {case:?}: a refused removal leaves every stored pair where it was, got \
                 {reported:?}"
            );
        }

        // The SAME route, with the parameter supplied: it serves, and it removes exactly the
        // key it names — so the refusals above reported a bad request, not a missing route.
        let case = format!("{} / the parameter supplied", subject.label());
        let target = format!("{}?tagKeys={}", subject.path(), pct(REMOVED_KEY));
        let (status, code, body) = emulator.send("DELETE", &target, None).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?} (DELETE {target}): the route is implemented and serves the removal, \
             got {status} {code:?}: {body}"
        );
        let mut want = starting_tags();
        want.remove(REMOVED_KEY);
        let reported = subject.reported_tags(&emulator, &case).await;
        assert_eq!(
            reported, want,
            "case {case:?}: the removal takes exactly the key it names off, got {reported:?}"
        );
    }
}

/// It is the parameter's NAME that routes a tag removal: supplied carrying no value at all,
/// the route matches and the request is served on its merits — never refused for an absent
/// member — and a removal that names no key the subject carries takes nothing away.
#[tokio::test]
async fn a_tag_keys_parameter_with_no_value_routes_a_tag_removal() {
    let emulator = Emulator::new();
    let subjects = tagged_subjects(&emulator).await;

    let member_path = aws_smithy_schema_server::constraints::path::member("TagKeys");
    let refusal = mc::validation_failed_message(&missing_member_clause(&member_path));

    for subject in &subjects {
        let case = format!(
            "{} / the parameter present carrying no value",
            subject.label()
        );
        let target = format!("{}?tagKeys=", subject.path());
        let (status, code, body) = emulator.send("DELETE", &target, None).await;
        assert_ne!(
            status, 501,
            "case {case:?} (DELETE {target}): a supplied parameter routes to the implemented \
             operation, got {status} {code:?}: {body}"
        );
        let message = message_of(&body);
        assert_ne!(
            message, refusal,
            "case {case:?}: a parameter the request supplies is not refused as absent, got \
             {status} {code:?}: {body}"
        );

        // Whichever way that request was answered, it named no key the subject carries, so
        // every stored pair is still there.
        let reported = subject.reported_tags(&emulator, &case).await;
        assert_eq!(
            reported,
            starting_tags(),
            "case {case:?}: a removal naming no carried key takes nothing away, got \
             {reported:?}"
        );
    }
}
