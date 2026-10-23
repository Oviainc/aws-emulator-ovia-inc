// SPDX-License-Identifier: CC0-1.0

//! Every form of this service's `X-Amz-Target` prefix reaches the same operation.
//!
//! The rule under test: on the `awsJson` protocols a request names its operation out of
//! band, in `x-amz-target: <target prefix>.<OperationName>`. The prefix is OPAQUE text an
//! SDK copies out of its service model and MAY carry dots of its own — this service's is
//! its absolute shape id, `com.amazonaws.codeconnections.CodeConnections_20231201` —
//! while an operation name is a Smithy identifier (ASCII letters, digits and `_`) and so
//! never can. So the OPERATION a request names is the segment after the target's LAST
//! dot, and the SERVICE is the last dot-separated segment of what precedes it, whatever
//! the prefix in front of it.
//!
//! `wire_target_routing.rs` and `target_probe.rs` pin that for the two prefix forms an
//! SDK and this crate's other suites send. This suite asserts it over the CLASS the rule
//! generalises across: prefix forms differing in how many dot-separated labels they carry
//! (none, one, three, five) and in what those labels are, driven over the whole modelled
//! operation surface, over a resource round trip, and — because a request carrying no
//! SigV4 credential scope is routed by its target alone — signed and unsigned.
//!
//! Everything goes through the real `MultiServiceRouter` dispatch, so what is asserted is
//! what a client sees.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, call_with_target, router};
use super::git_sync::error_code;
use super::target_probe::OPERATIONS;

/// The `X-Amz-Target` prefix forms that name THIS service, each `(case, prefix)`.
///
/// The service shape is `com.amazonaws.codeconnections#CodeConnections_20231201`
/// (`smithy_query(query="service_info")`), so its local name `CodeConnections_20231201`
/// is the last label of every row; the rows differ in what stands in front of it, which
/// is the only thing the rule generalises over.
const PREFIXES: &[(&str, &str)] = &[
    ("the bare service shape name", "CodeConnections_20231201"),
    (
        "the absolute shape id an AWS SDK sends",
        "com.amazonaws.codeconnections.CodeConnections_20231201",
    ),
    (
        "one namespace label before the shape name",
        "codeconnections.CodeConnections_20231201",
    ),
    (
        // The form a Coral service name takes when the namespace carries an API-version
        // label of its own (CloudTrail publishes exactly this shape of prefix).
        "a version label inside the namespace",
        "com.amazonaws.codeconnections.v20231201.CodeConnections_20231201",
    ),
    (
        "a deeper namespace than any SDK sends",
        "a.b.c.d.e.CodeConnections_20231201",
    ),
];

/// Targets that name NO operation of this service, each `(case, whole target)`.
///
/// Each one carries a DOTTED prefix: under the framework's first-dot reading its routing
/// key would be a `<namespace tail>.<Service>.<Operation>` string, which names no
/// operation of any service — so resolving the operation from the last dot must not turn
/// one of these into a handler call.
const UNSERVED_TARGETS: &[(&str, &str)] = &[
    (
        "another service's dotted prefix",
        "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
    ),
    (
        "another service's dotted prefix with one of THIS service's operation names",
        "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.ListConnections",
    ),
    (
        "a last label that merely ends in this shape name's characters",
        "com.amazonaws.codeconnections.NotCodeConnections_20231201.ListConnections",
    ),
    (
        "an operation this model does not bind",
        "com.amazonaws.codeconnections.CodeConnections_20231201.NoSuchOperation",
    ),
    (
        "nothing after the last dot",
        "com.amazonaws.codeconnections.CodeConnections_20231201.",
    ),
];

/// The framework's unrouted-501 wording for `target`: it names the routing key it could
/// not resolve, which is everything after the target's FIRST dot. That message is the
/// marker of a request that reached no operation at all — the scaffold channel's own 501,
/// for an operation nobody has implemented yet, names the operation by its short name and
/// says it is `scaffolded but not implemented`.
fn unrouted_message(target: &str) -> String {
    let key = target
        .split_once('.')
        .map_or_else(|| target.to_string(), |(_, rest)| rest.to_string());
    format!("operation {key} is not implemented")
}

/// Call `target` WITHOUT a SigV4 credential scope, so the request's only routable signal
/// is the target itself.
async fn call_unsigned(
    router: &MultiServiceRouter,
    target: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "target {target}: body is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// EVERY modelled operation is reached behind EVERY prefix form that names this service.
///
/// Whatever the operation then answers — its output, a modelled failure, or the request
/// walk refusing an empty body — it is the operation answering, not the dispatcher
/// refusing a key it could not resolve. The one 501 a routed request may get is the
/// scaffold channel's, which names the operation by its short name.
#[tokio::test]
async fn every_modelled_operation_is_reached_behind_every_prefix_form() {
    for (form, prefix) in PREFIXES {
        let router = router();
        for operation in OPERATIONS {
            let target = format!("{prefix}.{operation}");
            let (status, body) = call_with_target(&router, &target, serde_json::json!({})).await;
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                !message.contains(&unrouted_message(&target)),
                "case {form:?}, {operation}: the target must reach the operation, got \
                 {status} {body}"
            );
            if error_code(&body) == "NotImplemented" {
                assert!(
                    message.contains(&format!("operation {operation} is scaffolded")),
                    "case {form:?}, {operation}: a 501 here may only be the scaffold \
                     channel, got {status} {body}"
                );
            }
        }
    }
}

/// The prefix form changes neither the operation a request reaches nor the state it reads:
/// a connection created behind one form reads back behind every other, out of the one
/// store, and the list reports every connection whichever form enumerates it.
#[tokio::test]
async fn one_store_lies_behind_every_prefix_form() {
    let router = router();

    let mut created: Vec<(&str, String)> = Vec::new();
    for (index, (form, prefix)) in PREFIXES.iter().enumerate() {
        // One connection per writing form, named after it, so a read can tell which form
        // stored which row.
        let name = format!("prefix-form-{index}");
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.CreateConnection"),
            serde_json::json!({ "ConnectionName": name, "ProviderType": "GitHub" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}: expected 200 from CreateConnection, got {status} {body}"
        );
        let arn = body["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {form:?}: no ConnectionArn: {body}"))
            .to_string();
        assert!(
            arn.contains(REGION),
            "case {form:?}: expected the ARN minted in {REGION}, got {arn:?}"
        );
        created.push((form, arn));
    }

    // Read every connection back behind EVERY form, including the ones that did not
    // write it.
    for (writing_form, arn) in &created {
        for (reading_form, prefix) in PREFIXES {
            let (status, body) = call_with_target(
                &router,
                &format!("{prefix}.GetConnection"),
                serde_json::json!({ "ConnectionArn": arn }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {writing_form:?} written, read behind {reading_form:?}: expected 200 \
                 from GetConnection, got {status} {body}"
            );
            assert_eq!(
                body["Connection"]["ConnectionArn"].as_str(),
                Some(arn.as_str()),
                "case {writing_form:?} written, read behind {reading_form:?}: expected the \
                 connection {arn:?}, got {body}"
            );
        }
    }

    // One store behind all the forms: each enumeration reports every connection written.
    let expected: Vec<&str> = {
        let mut arns: Vec<&str> = created.iter().map(|(_, arn)| arn.as_str()).collect();
        arns.sort_unstable();
        arns
    };
    for (form, prefix) in PREFIXES {
        let (status, body) = call_with_target(
            &router,
            &format!("{prefix}.ListConnections"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}: expected 200 from ListConnections, got {status} {body}"
        );
        let mut listed: Vec<&str> = body["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {form:?}: no Connections list: {body}"))
            .iter()
            .filter_map(|connection| connection["ConnectionArn"].as_str())
            .collect();
        listed.sort_unstable();
        assert_eq!(
            listed, expected,
            "case {form:?}: expected every connection written behind every form, got {body}"
        );
    }
}

/// A request whose ONLY routable signal is its `X-Amz-Target` reaches this service, for
/// every prefix form — and only when the target names THIS service.
///
/// With no credential scope the framework's routing key is the target's FIRST label,
/// which for a dotted prefix is a namespace label (`com`) naming no service, so such a
/// request would otherwise reach nobody and answer `404 UnknownService`.
#[tokio::test]
async fn an_unsigned_request_is_routed_by_the_service_its_target_names() {
    let router = router();
    for (form, prefix) in PREFIXES {
        let (status, body) = call_unsigned(
            &router,
            &format!("{prefix}.ListConnections"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}, unsigned: expected 200 from ListConnections, got {status} {body}"
        );
        assert!(
            body["Connections"].is_array(),
            "case {form:?}, unsigned: expected a Connections list, got {body}"
        );
    }

    // A target naming another service is not served by this one, signed or not.
    for (case, target) in UNSERVED_TARGETS {
        let (status, body) = call_unsigned(&router, target, serde_json::json!({})).await;
        assert_ne!(
            status, 200,
            "case {case:?}, unsigned: this service must not serve {target:?}, got {body}"
        );
    }
}

/// Resolving the operation from the last dot invents no handlers: a target that names
/// another service, or an operation this model does not bind, is still refused before any
/// handler runs.
#[tokio::test]
async fn a_target_naming_no_operation_of_this_service_is_refused() {
    let router = router();
    for (case, target) in UNSERVED_TARGETS {
        let (status, body) = call_with_target(&router, target, serde_json::json!({})).await;
        assert_eq!(
            (status, error_code(&body)),
            (501, "NotImplemented"),
            "case {case:?}: expected the framework's routing refusal for {target:?}, got \
             {status} {body}"
        );
    }
}

/// A prefix spelled LOOSELY — the API-version suffix dropped, or cased differently from
/// the model — is claimed by this service (the framework resolves its routing key that
/// way) and then answered by the FRAMEWORK, never by a handler this rule guessed.
///
/// `service_common::rpc_target` resolves an operation only for a prefix whose service
/// segment is spelled as the model spells its shape name, since that answer indexes the
/// generated routing table; so a loose prefix falls through, and what a client sees is
/// the framework's own first-dot routing: the operation name alone for a
/// single-identifier prefix (which routes, exactly as it did before this rule existed)
/// and the protocol's routing refusal for a dotted one.
#[tokio::test]
async fn a_loosely_spelled_prefix_is_answered_by_the_framework() {
    let router = router();

    // (case, target, whether the framework's own first-dot read still routes it)
    let rows: &[(&str, &str, bool)] = &[
        (
            "the version suffix dropped, single-identifier prefix",
            "CodeConnections.ListConnections",
            true,
        ),
        (
            "the shape name lower-cased, single-identifier prefix",
            "codeconnections_20231201.ListConnections",
            true,
        ),
        (
            "the version suffix dropped, dotted prefix",
            "com.amazonaws.codeconnections.CodeConnections.ListConnections",
            false,
        ),
        (
            "the shape name lower-cased, dotted prefix",
            "com.amazonaws.codeconnections.codeconnections_20231201.ListConnections",
            false,
        ),
    ];

    for (case, target, routed_by_the_framework) in rows {
        let (status, body) = call_with_target(&router, target, serde_json::json!({})).await;
        if *routed_by_the_framework {
            assert_eq!(
                status, 200,
                "case {case:?}: the framework's own routing key is the operation name here, \
                 so {target:?} must still reach ListConnections, got {status} {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (501, "NotImplemented"),
                "case {case:?}: a prefix no model declares must get the framework's routing \
                 refusal, not a handler, got {status} {body}"
            );
        }
    }
}
