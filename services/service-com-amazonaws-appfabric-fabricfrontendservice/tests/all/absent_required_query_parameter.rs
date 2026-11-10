// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request whose method and path address a modelled route but
//! which binds no value for a `@required` member the model binds to the QUERY string.
//!
//! RULE. A request whose method and path address a route this model binds is served by the
//! operation bound to that route, so the request's OWN values decide the answer: a
//! `@required` member it carries no value for is refused with the modelled
//! `ValidationException` (HTTP 400, `reason` `fieldValidationFailed`, `fieldList` naming
//! that member), wherever the model binds the member — path, body or query string. It is
//! never reported as an operation nobody implemented. The framework's REST router rejects a
//! candidate route whose `@required` header/query bindings are absent from the request
//! (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`), which is how a
//! model that overloads one method + URI template across several operations tells them
//! apart by the shape of the request; this model overloads no route, so for it that check
//! could only turn a refusable request into HTTP 501, a status no operation here models.
//!
//! The tables span what the rule generalises over rather than one recorded call:
//!
//! * every route of this model that binds a `@required` member to the query string, DERIVED
//!   FROM THE MODEL by walking it, so a route the model adds to that class is covered the
//!   day it arrives;
//! * every shape "the parameter is absent" takes on the wire — no query string at all, an
//!   empty one, only another parameter, a parameter whose name merely starts with the bound
//!   one, the bound name in another case, the bound name as another parameter's VALUE, and
//!   several unrelated parameters — because the router compares parameter NAMES, so a query
//!   string carrying no pair named by the binding is absent however much else it carries;
//! * each resource kind the tag routes address, each carrying a tag set read back through
//!   the implemented report, so a refusal that quietly wrote the store cannot pass;
//! * the contrasts the rule turns on: the same route WITH the parameter, which serves; a
//!   parameter present but carrying a value the operation refuses, which is the operation's
//!   own answer and not this refusal; and a method and path the model binds NO route for,
//!   which keeps the framework's own routing answer.
//!
//! Every case drives the emulator the way a client does — a buffered `restJson1` request
//! through the same router the production binary assembles — on the harness
//! `app_bundle_ingestion_lifecycle` owns.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{encode, unknown_bundle_arn, Answer, Emulator};
use crate::pagination_token_lifetime::message;
use crate::resource_tags::{expected, list_tags, reported_tags, tag_list, taggable_resources};
use crate::validation_refusal_shape::{field_list, reason};

/// This service's Smithy model, so the derived rows are the model's own rather than a copy
/// of it that can drift.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json"
));

/// The operations this model binds a `@required` member to the query string on, from
/// `smithy_query(query="operation", operation="UntagResource")`
/// (`UntagResource$tagKeys`, `@httpQuery tagKeys`). Asserted to be among what the walk
/// below finds, so a walk that silently stopped finding routes cannot pass.
const KNOWN_REQUIRED_QUERY_OPERATIONS: &[&str] = &["UntagResource"];

/// The error code the model declares for a request this service does not accept
/// (`ValidationException`, `@httpError 400`), which every operation of this model lists.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The modelled `ValidationExceptionReason` for a request whose NAMED member the service
/// read and does not accept.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The tag keys every subject of the tag table starts out carrying: more than one, so a
/// refusal that removed some of them and a removal that removed all of them are both
/// visible.
fn starting_tags() -> Vec<(&'static str, &'static str)> {
    vec![("env", "prod"), ("team", "platform"), ("owner", "ops")]
}

/// The one key the serving removal names, and so the only key that stops being carried.
const REMOVED_KEY: &str = "env";

/// The clause this service states for a member the request bound no value for, at the
/// member path `at`: the framework's own wording, shared with every other absent
/// `@required` member of this service (a body member, a path label), so this refusal and
/// those cannot drift apart.
fn absent_member_clause(at: &str) -> String {
    format!("Value null at '{at}' failed to satisfy constraint: Member must not be null")
}

/// The `ValidationException` message over ONE clause: the AWS envelope stating how many
/// constraints the request violated, then the clause.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The shapes "the request binds no value for the parameter" takes on the wire, each as
/// `(case, query)` — appended to a route's path as written.
///
/// The router reads raw `key[=value]` pairs off the query string and compares their NAMES,
/// so no row below carries a pair named `tagKeys`, whatever else it carries, and every row
/// must be answered the same way.
const ABSENT_QUERIES: &[(&str, &str)] = &[
    ("no query string at all", ""),
    ("an empty query string", "?"),
    ("a query carrying only another parameter", "?maxResults=1"),
    (
        "a parameter whose name only starts with the bound one",
        "?tagKeysExtra=env",
    ),
    (
        "a parameter whose name only ends with the bound one",
        "?extraTagKeys=env",
    ),
    ("the bound name in another case", "?TagKeys=env"),
    ("the bound name as another parameter's value", "?filter=tagKeys"),
    ("several unrelated parameters", "?first=1&second=2&third=3"),
];

/// One route of this model whose input binds a `@required` member to the query string, as
/// the model declares it.
#[derive(Debug)]
struct RequiredQueryRoute {
    /// The operation's shape name.
    operation: String,
    /// Its `@http` method.
    method: String,
    /// Its `@http` URI with every `@httpLabel` filled, and no query string.
    target: String,
    /// Each `@required` + `@httpQuery` member of its input: the parameter name on the wire,
    /// and the member path a refusal states it by.
    parameters: Vec<(String, String)>,
}

/// Every route of this model whose input binds a `@required` member to the query string, in
/// operation order.
///
/// Read off the model: an operation shape carrying `@http`, whose input declares a member
/// with BOTH `smithy.api#required` and `smithy.api#httpQuery`. No method, URI template or
/// parameter name is written out here, so a model that moves a route, renames a parameter
/// or adds an operation to this class moves the rows with it.
fn required_query_routes() -> Vec<RequiredQueryRoute> {
    let model: Value = serde_json::from_str(MODEL).expect("this service's Smithy model parses");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model declares shapes")
        .clone();

    let mut routes: Vec<RequiredQueryRoute> = Vec::new();
    for (shape_id, shape) in &shapes {
        if shape["type"].as_str() != Some("operation") {
            continue;
        }
        let Some(http) = shape["traits"].get("smithy.api#http") else {
            continue;
        };
        let Some(input) = shape["input"]["target"]
            .as_str()
            .and_then(|target| shapes.get(target))
        else {
            continue;
        };
        let mut parameters: Vec<(String, String)> = Vec::new();
        for (member_name, member) in input["members"].as_object().into_iter().flatten() {
            if member["traits"].get("smithy.api#required").is_none() {
                continue;
            }
            let Some(query) = member["traits"].get("smithy.api#httpQuery") else {
                continue;
            };
            // A `@httpQuery` trait carries the wire name; one with no value names the
            // member itself, which is how the router's own binding table reads it.
            let parameter = query.as_str().unwrap_or(member_name.as_str()).to_string();
            parameters.push((
                parameter,
                // The spelling a refusal states the member by, from the framework's own
                // path renderer rather than a second copy of its casing rule.
                aws_smithy_schema_server::constraints::path::member(member_name),
            ));
        }
        if parameters.is_empty() {
            continue;
        }
        routes.push(RequiredQueryRoute {
            operation: shape_id
                .rsplit('#')
                .next()
                .unwrap_or(shape_id.as_str())
                .to_string(),
            method: http["method"]
                .as_str()
                .expect("an @http trait states its method")
                .to_string(),
            target: with_labels_filled(
                http["uri"].as_str().expect("an @http trait states its uri"),
            ),
            parameters,
        });
    }
    routes.sort_by(|left, right| left.operation.cmp(&right.operation));
    routes
}

/// `uri_pattern` with every `{label}` / `{label+}` segment replaced by an ARN no store
/// holds, percent-encoded into ONE path segment the way an AWS SDK encodes a label, and any
/// static query literal of the pattern dropped — the request target a client sends for that
/// route.
///
/// An ARN satisfies both label shapes this model declares (`Arn`'s `^arn:.+$` and
/// `Identifier`'s ARN branch), so the derived request violates no constraint other than the
/// absent parameter under test. What the label ADDRESSES never decides the answer, because
/// the refusal is raised at the request boundary — which the tag table below pins against
/// resources that are really there.
fn with_labels_filled(uri_pattern: &str) -> String {
    let encoded = encode(&unknown_bundle_arn("11111111-2222-3333-4444-555555555555"));
    let path = uri_pattern.split('?').next().unwrap_or(uri_pattern);
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                encoded.clone()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<String>>()
        .join("/")
}

/// `target` with `parameter` carrying `value` appended to its query string.
fn with_parameter(target: &str, parameter: &str, value: &str) -> String {
    match target.contains('?') {
        true => format!("{target}&{parameter}={value}"),
        false => format!("{target}?{parameter}={value}"),
    }
}

/// Assert `answer` is the modelled refusal of a request that bound no value for the member
/// at `at`: the status and code the model gives `ValidationException`, the modelled
/// `reason` for a faulted member, the one-violation envelope naming it, and a `fieldList`
/// entry for that member alone.
fn assert_absent_member_refusal(case: &str, at: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, VALIDATION_EXCEPTION),
        "case {case:?}: a request binding no value for {at:?} is refused as a client error, \
         got {} {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        reason(answer),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {}",
        answer.body
    );
    let stated = message(answer);
    let want = one_violation(&absent_member_clause(at));
    assert_eq!(
        stated, want,
        "case {case:?}: expected the message {want:?}, got {stated:?}"
    );
    let fields = field_list(answer);
    assert_eq!(
        fields,
        vec![(at.to_string(), absent_member_clause(at))],
        "case {case:?}: expected a fieldList naming {at:?} alone, got {fields:?}"
    );
}

/// `DELETE` on a tag route, with `query` appended to the path exactly as written.
async fn delete_tags(emulator: &Emulator, arn: &str, query: &str) -> Answer {
    emulator
        .call("DELETE", &format!("/tags/{}{query}", encode(arn)), None)
        .await
}

/// RULE: every route of this model that binds a `@required` member to the query string
/// refuses a request that binds no value for that member — the modelled
/// `ValidationException` naming the member — and serves the same route once the parameter
/// is supplied, so the absent parameter is answered as the client error it is and never as
/// an unimplemented operation.
#[tokio::test]
async fn every_modelled_required_query_parameter_left_out_is_the_modelled_validation_exception() {
    let emulator = Emulator::new();
    let routes = required_query_routes();

    let found: Vec<&str> = routes
        .iter()
        .map(|route| route.operation.as_str())
        .collect();
    for operation in KNOWN_REQUIRED_QUERY_OPERATIONS {
        assert!(
            found.contains(operation),
            "case {operation:?}: the model walk must find every operation binding a required \
             query parameter, found {found:?}"
        );
    }

    for route in &routes {
        for (row, query) in ABSENT_QUERIES {
            let case = format!("{} {} {}{row}", route.operation, route.method, route.target);
            let target = format!("{}{query}", route.target);
            let answer = emulator.call(&route.method, &target, None).await;
            assert_ne!(
                answer.status, 501,
                "case {case:?} ({} {target}): the operation is implemented, so its route is \
                 never reported unimplemented, got {}",
                route.method, answer.body
            );
            for (_, at) in &route.parameters {
                assert_absent_member_refusal(&case, at, &answer);
            }
        }

        // The SAME route with every parameter supplied: it reaches the operation, so
        // whatever it is answered with, it is not refused for an absent member.
        let case = format!(
            "{} {} {} / every parameter supplied",
            route.operation, route.method, route.target
        );
        let mut supplied = route.target.clone();
        for (parameter, _) in &route.parameters {
            supplied = with_parameter(&supplied, parameter, "env");
        }
        let answer = emulator.call(&route.method, &supplied, None).await;
        assert_ne!(
            answer.status, 501,
            "case {case:?}: a request carrying its required parameter reaches the operation, \
             got {}",
            answer.body
        );
        let stated = message(&answer);
        for (_, at) in &route.parameters {
            assert!(
                !stated.contains(&absent_member_clause(at)),
                "case {case:?}: a request that supplies {at:?} is not refused for its absence, \
                 got {} {stated:?}",
                answer.status
            );
        }
    }
}

/// RULE (the same one, against resources that really carry tags): a tag removal binding no
/// value for `tagKeys` is the modelled refusal naming that member, for every resource kind
/// and whatever else its query string carries; because it is refused before the handler
/// runs it leaves every stored pair where it was; and the very same route removes exactly
/// the key it names once the parameter is there.
#[tokio::test]
async fn a_tag_removal_binding_no_tag_keys_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let held = starting_tags();
    let subjects = taggable_resources(&emulator, tag_list(&held)).await;
    let at = aws_smithy_schema_server::constraints::path::member("tagKeys");

    for (kind, arn) in &subjects {
        for (row, query) in ABSENT_QUERIES {
            let case = format!("{row} on an {kind}");
            let answer = delete_tags(&emulator, arn, query).await;
            assert_absent_member_refusal(&case, &at, &answer);

            // Refused before the handler ran, so the stored set is untouched.
            let listed = list_tags(&emulator, arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected(&held),
                "case {case:?}: a refused removal leaves every stored pair where it was, got {}",
                listed.body
            );
        }

        // The SAME route with the parameter supplied: it serves, and it removes exactly the
        // key it names — so the refusals above reported a bad request, not a missing route.
        let case = format!("the parameter supplied on an {kind}");
        let answer = delete_tags(&emulator, arn, &format!("?tagKeys={}", encode(REMOVED_KEY))).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the route is implemented and serves the removal, got {} {}",
            answer.status, answer.body
        );
        let left: Vec<(&str, &str)> = held
            .iter()
            .copied()
            .filter(|(key, _)| *key != REMOVED_KEY)
            .collect();
        let listed = list_tags(&emulator, arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            expected(&left),
            "case {case:?}: the removal takes exactly the key it names off, got {}",
            listed.body
        );
    }
}

/// RULE: it is the parameter's NAME that routes the request, so a request that supplies
/// `tagKeys` is answered on its own merits — never refused for an absent member, never
/// reported unimplemented — whatever value it carries.
///
/// The rows span the values a supplied parameter can carry: no value at all, a key the
/// resource does not hold, the key it does hold, and the parameter repeated once per key
/// (the wire form an AWS SDK gives a query-bound list).
#[tokio::test]
async fn a_supplied_tag_keys_parameter_routes_the_removal_whatever_it_carries() {
    let emulator = Emulator::new();
    let held = starting_tags();
    let subjects = taggable_resources(&emulator, tag_list(&held)).await;
    let at = aws_smithy_schema_server::constraints::path::member("tagKeys");
    let refusal = one_violation(&absent_member_clause(&at));

    // (case, query, the keys the call takes off)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        ("the parameter carrying no value", "?tagKeys=".to_string(), vec![]),
        (
            "a key the resource does not hold",
            "?tagKeys=absent-key".to_string(),
            vec![],
        ),
        (
            "one key the resource holds",
            format!("?tagKeys={}", encode("env")),
            vec!["env"],
        ),
        (
            "the parameter repeated once per key",
            format!("?tagKeys={}&tagKeys={}", encode("team"), encode("owner")),
            vec!["team", "owner"],
        ),
    ];

    for (kind, arn) in &subjects {
        let mut removed: Vec<&str> = Vec::new();
        for (row, query, takes) in &rows {
            let case = format!("{row} on an {kind}");
            let answer = delete_tags(&emulator, arn, query).await;
            assert_ne!(
                answer.status, 501,
                "case {case:?}: a supplied parameter routes to the implemented operation, got {}",
                answer.body
            );
            let stated = message(&answer);
            assert_ne!(
                stated, refusal,
                "case {case:?}: a parameter the request supplies is not refused as absent, got \
                 {} {}",
                answer.status, answer.body
            );

            removed.extend(takes.iter().copied());
            let left: Vec<(&str, &str)> = held
                .iter()
                .copied()
                .filter(|(key, _)| !removed.contains(key))
                .collect();
            let listed = list_tags(&emulator, arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected(&left),
                "case {case:?}: the stored set must be exactly what the named keys left, got {}",
                listed.body
            );
        }
    }
}

/// RULE (the other side): only a route the model BINDS is served this way. A method and
/// path this model binds no route for still gets the framework's own routing answer, so
/// naming the addressed operation above claims no route of its own.
#[tokio::test]
async fn a_method_and_path_the_model_binds_no_route_for_keeps_the_frameworks_answer() {
    let emulator = Emulator::new();
    let bundle = crate::app_bundle_ingestion_lifecycle::arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let label = encode(&bundle);

    // (case, method, target) — each differs from a bound route in one way: the method, a
    // path segment too few, a path segment too many, and a path space the model has none of.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a method the model binds on no tag route",
            "PUT",
            format!("/tags/{label}"),
        ),
        ("the tag route without its label", "DELETE", "/tags".to_string()),
        (
            "a path segment past the tag route",
            "DELETE",
            format!("/tags/{label}/keys"),
        ),
        (
            "a path space this model does not publish",
            "DELETE",
            "/nothing-this-model-binds".to_string(),
        ),
    ];

    for (case, method, target) in rows {
        let answer = emulator.call(method, &target, None).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?} ({method} {target}): a request addressing no modelled route keeps \
             the framework's routing answer, got {} {}",
            answer.status, answer.body
        );
    }

    // And the store is still what the create left: none of the above reached a handler.
    let listed = list_tags(&emulator, &bundle).await;
    assert_eq!(
        reported_tags(&listed, "after the unbound routes"),
        expected(&[]),
        "an unbound route changes nothing a later caller sees, got {}",
        listed.body
    );
}
