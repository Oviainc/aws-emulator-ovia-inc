// SPDX-License-Identifier: CC0-1.0

//! WHERE this service refuses a value that breaks a modelled member constraint,
//! end to end over the wire (the shared harness is `crate::wire`).
//!
//! One rule, over a table spanning the class: a value outside a constraint the
//! model declares for its member is refused in exactly ONE place — the front door
//! every modelled operation is served behind — which places the refusal by the
//! member's own HTTP binding (a `@httpQuery` / `@httpLabel` / `@httpHeader` value
//! keeps the modelled `ValidationException`, with the AWS envelope message and the
//! `fieldList` naming each refused member; a body value earns the front door's
//! `BadRequestException`). No operation states that refusal a second time for
//! itself, so the answer never depends on which operation the request named.
//!
//! The rows drive BOTH halves of that one path: `ListServices`, whose two inputs
//! are the service's whole URI-bound surface (`maxResults` `@range(1, 1000)` and
//! `nextToken` `@length(32, 512)` / `@pattern ^[-A-Za-z0-9+/_]+=*$` —
//! `smithy_query(query="operation", operation="ListServices")`), and
//! `UpdateAccountCustomizations`, whose inputs all travel in the body
//! (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`).
//! They differ along which operation is called, where the refused value travelled,
//! which KIND of constraint it broke, and how many violations one request carries.

use crate::wire::{build_router, call, encode_query_value, list_page, update_customizations};
use serde_json::{json, Value};

/// The error code this service's model declares for an input that fails its
/// constraints (`smithy_query(query="operation", operation="ListServices")`),
/// which a URI-bound refusal keeps.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code the front door names for a request BODY outside a modelled
/// constraint — not a shape this service's model declares, because the refusal
/// happens before the application layer that owns the modelled errors.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What the AWS envelope a member-constraint refusal states says after the count
/// of violations it detected (`N validation error[s] detected: …`), split so a row
/// is not asserted against one count's plural form.
const ENVELOPE_SUBJECT: &str = "validation error";
const ENVELOPE_VERB: &str = "detected:";

/// The marker each requirement clause states the broken constraint after.
const CONSTRAINT_CLAUSE: &str = "failed to satisfy constraint:";

/// The published bounds of `maxResults`' modelled `@range`
/// (`smithy_query(query="operation", operation="ListServices")`).
const MAX_RESULTS_RANGE: std::ops::RangeInclusive<i64> = 1..=1000;

/// Where the refused value of one row travelled, which is what decides the
/// channel the single front door answers it through.
enum Refused {
    /// A `ListServices` request carrying this raw query string.
    UriBound { query: String, members: Vec<&'static str> },
    /// An `UpdateAccountCustomizations` request carrying this body.
    BodyBound(Value),
}

/// One row: a request outside a modelled constraint, and where its value
/// travelled.
struct Case {
    case: String,
    refused: Refused,
}

/// Every row the rule is asserted over.
fn cases() -> Vec<Case> {
    let mut rows: Vec<Case> = Vec::new();

    // `maxResults` outside its `@range`: each published bound crossed by one, and
    // far past it at both ends.
    for max_results in [0_i64, -1, -9999, 1001, 7500, 1_000_000] {
        assert!(
            !MAX_RESULTS_RANGE.contains(&max_results),
            "case maxResults={max_results}: a refused row must fall outside @range(1, 1000), \
             got a value the model admits"
        );
        rows.push(Case {
            case: format!("maxResults={max_results} outside its @range"),
            refused: Refused::UriBound {
                query: format!("maxResults={max_results}"),
                members: vec!["maxResults"],
            },
        });
    }

    // `nextToken` outside its `@length`, inside its charset so length is the only
    // constraint broken: each published bound crossed by one, and far past it.
    for length in [1_usize, 31, 513, 2048] {
        let token = "A".repeat(length);
        rows.push(Case {
            case: format!("a {length}-character nextToken outside its @length"),
            refused: Refused::UriBound {
                query: format!("nextToken={}", encode_query_value(&token)),
                members: vec!["nextToken"],
            },
        });
    }

    // `nextToken` inside its `@length` but outside its `@pattern`.
    for (name, token) in [
        ("a run of illegal characters", "#".repeat(64)),
        ("one illegal character at the tail", format!("{}.", "A".repeat(63))),
    ] {
        rows.push(Case {
            case: format!("nextToken with {name}, outside its @pattern"),
            refused: Refused::UriBound {
                query: format!("nextToken={}", encode_query_value(&token)),
                members: vec!["nextToken"],
            },
        });
    }

    // Two URI-bound members refused in one request, so the row spans the count of
    // violations one answer reports as well as which member broke what.
    rows.push(Case {
        case: "maxResults and nextToken refused together".to_string(),
        refused: Refused::UriBound {
            query: format!("maxResults=0&nextToken={}", encode_query_value("short")),
            members: vec!["maxResults", "nextToken"],
        },
    });

    // The other half of the one path: the same front door, for the operation whose
    // members all travel in the BODY — an enum value the model does not declare, a
    // string outside its `@pattern`, and one outside its `@length`.
    for (name, body) in [
        ("an accountColor outside the modelled enum", json!({"accountColor": "chartreuse"})),
        (
            "a visibleServices member outside its @pattern",
            json!({"visibleServices": ["Not_A_Service!"]}),
        ),
        (
            "a visibleServices member past its @length maximum",
            json!({"visibleServices": ["a".repeat(65)]}),
        ),
        (
            "a visibleRegions member outside its @pattern",
            json!({"visibleRegions": ["US-EAST-1"]}),
        ),
    ] {
        rows.push(Case {
            case: format!("{name}, in the request body"),
            refused: Refused::BodyBound(body),
        });
    }

    rows
}

/// The `message` of an answer, which every refusal of this service carries.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must carry a message, got {body}"))
        .to_string()
}

/// The member paths an answer's `fieldList` names, or `None` when it carries none.
fn reported_members(case: &str, body: &Value) -> Option<Vec<String>> {
    let entries = body.get("fieldList")?.as_array().unwrap_or_else(|| {
        panic!("case {case:?}: fieldList must be an array, got {body}")
    });
    let mut paths: Vec<String> = entries
        .iter()
        .map(|entry| {
            entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a fieldList entry requires path, got {entry}")
                })
                .to_string()
        })
        .collect();
    paths.sort();
    Some(paths)
}

/// The rule: every modelled member constraint is refused by the one front door,
/// through the channel the member's binding places it in — and a value the
/// constraint admits is still served by the operation itself.
#[tokio::test]
async fn one_front_door_refuses_every_modelled_member_constraint() {
    let router = build_router();

    for Case { case, refused } in cases() {
        let case = case.as_str();
        let (status, error_type, body) = match &refused {
            Refused::UriBound { query, .. } => {
                call(&router, "GET", &format!("/v1/services?{query}"), None).await
            }
            Refused::BodyBound(document) => update_customizations(&router, document.clone()).await,
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({body})"
        );
        let stated = message(case, &body);

        match &refused {
            // A URI-bound value keeps the modelled exception: the AWS envelope,
            // counting the violations, and one `fieldList` entry per refused
            // member stating that member's own clause.
            Refused::UriBound { members, .. } => {
                assert_eq!(
                    error_type, VALIDATION_EXCEPTION,
                    "case {case:?}: a URI-bound refusal keeps {VALIDATION_EXCEPTION}, \
                     got {error_type:?} (message {stated:?})"
                );
                let count = stated
                    .split(' ')
                    .next()
                    .and_then(|head| head.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the one refusal states the AWS envelope, which \
                             opens with the count of violations, got {stated:?}"
                        )
                    });
                assert!(
                    stated.contains(ENVELOPE_SUBJECT)
                        && stated.contains(ENVELOPE_VERB)
                        && stated.contains(CONSTRAINT_CLAUSE),
                    "case {case:?}: expected the envelope stating {ENVELOPE_SUBJECT:?} \
                     {ENVELOPE_VERB:?} and the broken constraint, got {stated:?}"
                );
                let mut expected: Vec<String> =
                    members.iter().map(|member| member.to_string()).collect();
                expected.sort();
                let reported = reported_members(case, &body).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a member-constraint refusal names its members in \
                         fieldList, got {body}"
                    )
                });
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the refused members reported, got {reported:?} \
                     (message {stated:?})"
                );
                assert!(
                    count >= expected.len(),
                    "case {case:?}: the envelope must count at least one violation per refused \
                     member, got {count} for {reported:?}"
                );
                for member in members {
                    assert!(
                        stated.contains(&format!("at '{member}'")),
                        "case {case:?}: the message must name the refused member {member:?}, \
                         got {stated:?}"
                    );
                }
            }
            // A body value earns the front door's own code and its one message,
            // which names the class of failure rather than a member.
            Refused::BodyBound(_) => {
                assert_eq!(
                    error_type, BAD_REQUEST_EXCEPTION,
                    "case {case:?}: a body refusal is the front door's {BAD_REQUEST_EXCEPTION}, \
                     got {error_type:?} (message {stated:?})"
                );
                assert!(
                    reported_members(case, &body).is_none(),
                    "case {case:?}: a body refusal names no member, got {body}"
                );
            }
        }
    }

    // The other side: a value the constraint ADMITS is not refused anywhere, and
    // the operation serves it — each published bound of the one `@range`, and the
    // values just inside them, size the page they ask for.
    for max_results in [*MAX_RESULTS_RANGE.start(), 2, 999, *MAX_RESULTS_RANGE.end()] {
        let (services, _) = list_page(&router, Some(max_results), None).await;
        let asked = usize::try_from(max_results).expect("a bound inside @range(1, 1000) is positive");
        assert!(
            services.len() <= asked,
            "case maxResults={max_results}: a page must hold at most what it asked for, \
             got {} entries",
            services.len()
        );
    }
}
