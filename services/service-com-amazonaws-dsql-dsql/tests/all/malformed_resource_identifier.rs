// SPDX-License-Identifier: CC0-1.0

//! What an identifier-addressed operation answers for a value that is no
//! identifier at all.
//!
//! RULE under test (one of two complements of
//! `tests/all/resource_identifier_absence.rs`, which pins what an identifier of
//! this service's own form gets): the OUTER form an identifier has to be in is
//! `ClusterId`'s and `StreamId`'s `^[a-z0-9]{26}$`
//! (`smithy_query(query="shape", shape="ClusterId,StreamId")`), so a value
//! outside that pattern names no resource and is refused as INPUT — HTTP 400 with
//! the service's own modelled `ValidationException`, whose `@required` `reason` is
//! filled and whose `fieldList` names the member the value arrived in — before any
//! state is read, and never as a report that some resource is absent. A value
//! INSIDE the pattern that the identifier encoding does not render is refused the
//! same way, which `tests/all/identifier_format_refusal.rs` pins.
//!
//! The value table spans the ways a URI label can fall outside the pattern: the
//! wrong case, the wrong length (under, over, and far under) and a character the
//! pattern's set leaves out. Both halves of the rule run over ONE route table,
//! the one `resource_identifier_absence::routes` spells, so every operation that
//! carries an identifier is asserted on the member it binds.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::Value;

use crate::harness::Dsql;
use crate::resource_identifier_absence::{routes, IdCase, IDENTIFIER_LEN};

/// Values OUTSIDE `^[a-z0-9]{26}$`: the ways a URI label can fail to be an
/// identifier at all — the wrong case, the wrong length, and a character the
/// pattern's set leaves out.
fn values_outside_the_pattern() -> Vec<IdCase> {
    // A row that IS inside the pattern, which every row below breaks in exactly
    // one way.
    let inside = "abcdefghjkmnpqrstvwxyz0123";
    assert_eq!(
        inside.len(),
        IDENTIFIER_LEN,
        "the base row must be a legal identifier, got {inside:?}"
    );
    let mut cases = vec![
        IdCase {
            name: "uppercase throughout".to_string(),
            value: inside.to_uppercase(),
        },
        IdCase {
            name: "one uppercase symbol".to_string(),
            value: format!("A{}", &inside[1..]),
        },
        IdCase {
            name: "one symbol short".to_string(),
            value: inside[..IDENTIFIER_LEN - 1].to_string(),
        },
        IdCase {
            name: "one symbol long".to_string(),
            value: format!("{inside}0"),
        },
        IdCase {
            name: "a single symbol".to_string(),
            value: "a".to_string(),
        },
    ];
    // Characters the pattern's set leaves out, each replacing one legal symbol so
    // the length stays right and only the character set is wrong.
    for punctuation in ['-', '_', '.', '!', '~'] {
        cases.push(IdCase {
            name: format!("{punctuation:?}, outside the pattern's character set"),
            value: format!("{punctuation}{}", &inside[1..]),
        });
    }
    cases
}

/// RULE: a value outside `^[a-z0-9]{26}$` is refused as input — 400
/// `ValidationException` with the modelled body, naming the member it arrived in —
/// on every route that carries a resource identifier.
#[tokio::test]
async fn a_value_outside_the_modeled_pattern_is_refused_as_input() {
    let dsql = Dsql::new();
    // One live cluster, so a row that puts the value under test in the stream
    // label is refused for that label rather than for a missing cluster.
    let live = dsql.create_settled().await;

    let routes = routes();
    for case in values_outside_the_pattern() {
        for route in &routes {
            let (method, path, body) = (route.request)(&case.value, &live);
            let answer = dsql.call(method, &path, body).await;
            let name = format!("{} on {}", case.name, route.operation);

            assert_eq!(
                answer.status, 400,
                "case {name}: a value outside the modelled pattern must answer 400, \
                 got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {name}: the refusal must be a ValidationException, got {:?}",
                answer.error
            );
            assert!(
                !answer.str("reason").is_empty(),
                "case {name}: ValidationException models reason as @required, got {:?}",
                answer.body
            );
            let field_list = answer.get("fieldList");
            let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
            assert!(
                fields
                    .iter()
                    .any(|field| field.get("name").and_then(Value::as_str) == Some(route.member)),
                "case {name}: the refusal must name {:?} in fieldList, got {field_list:?}",
                route.member
            );
            assert_eq!(
                answer.get("resourceId"),
                Value::Null,
                "case {name}: a refusal of the request names no resource, got {:?}",
                answer.body
            );
        }
    }

    // Nothing any refused call addressed was created, removed or changed: the one
    // live cluster is still the only one, still readable and still without a
    // policy.
    let read = dsql.get(&live).await;
    assert_eq!(
        read.status, 200,
        "the live cluster must be untouched by every refusal, got {} {:?}",
        read.status, read.body
    );
    let clusters = dsql.list("").await.get("clusters");
    assert_eq!(
        clusters.as_array().map(Vec::len),
        Some(1),
        "no refused request may have created a cluster, got {clusters:?}"
    );
    assert_eq!(
        dsql.get_policy(&live).await.status,
        404,
        "no refused request may have attached a policy"
    );
}
