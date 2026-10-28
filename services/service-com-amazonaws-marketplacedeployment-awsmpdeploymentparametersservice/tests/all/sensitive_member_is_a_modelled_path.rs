// SPDX-License-Identifier: CC0-1.0

//! WHICH member of a request is the `@sensitive` one — and therefore which refusal a
//! request is answered with — is decided by the model, never by text the caller chose.
//!
//! The rule under test: a validation report names a position as a trail from the
//! operation input, and only a segment contributed by a STRUCTURE member is a name the
//! model gave; a segment inside a collection is a value the caller sent (a map key, a
//! list position). So a member is `@sensitive` by its whole modelled trail, and a
//! collection entry whose key happens to be spelled like a member name is an ordinary
//! entry: it is held to its own shape's constraints, refused with the modelled HTTP 400
//! `ValidationException` naming its position, and its value is echoed like any other
//! entry's.
//!
//! On this model that separates two answers a client branches on:
//!
//! * `DeploymentParameterInput$secretString` (the model's only `sensitive: true` shape,
//!   `smithy_query(query="shape",shape="SecretString")`, reached as
//!   `deploymentParameter.secretString`) past its maximum is the front door's opaque
//!   403 `ForbiddenException`, and its value is never echoed;
//! * an entry of `PutDeploymentParameterRequest$tags` outside `TagValue`'s own bounds is
//!   the modelled 400 `ValidationException` at `tags.<the key the caller sent>`, for
//!   EVERY key `TagKey` admits — whose character class
//!   (`smithy_query(query="shape",shape="TagsMap")`: `TagKey`, `length 1..128`,
//!   `^[a-zA-Z0-9/_+=.:@-]+$`) admits the text of every member name of this model, and
//!   the `.` a nested trail is spelled with.
//!
//! The rows span what the rule generalises over: which member name the tag key imitates
//! (an input member, a nested member, the collection's own member, a member of an
//! output or of an error, a whole dotted trail), and which constraint class of
//! `TagValue` the entry breaks (maximum length, minimum length, character class).
//!
//! Every case drives the real wire path through the harness the `deployment_parameters`
//! suite owns, so what is asserted is what a signed SDK request observes.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::TAG_VALUE_MAX_LEN;

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, tag_map, tags_of,
    CATALOG, SECRET_MAX_LEN,
};

/// The status the modelled shape declares (`@httpError 400`).
const EXPECTED_STATUS: u16 = 400;
/// The wire error code it carries, written out rather than read from the crate so the
/// assertion is not the implementation compared with itself.
const EXPECTED_CODE: &str = "ValidationException";
/// The agreement and product every write below targets.
const AGREEMENT: &str = "agmt-sensitive-path";
const PRODUCT: &str = "prod-sensitive-path";
/// A secret inside every bound `SecretString` states, so no row below is answered for
/// the secret it carries instead of for the tag it carries.
const LEGAL_SECRET: &str = "s3cr3t";

/// Every member NAME this model spells, as a tag key may spell it.
///
/// Read off `smithy_query(query="closure",operation="PutDeploymentParameter")` and
/// `smithy_query(query="operation",operation="UntagResource")`: the input's own members,
/// the nested `DeploymentParameterInput` members, the output's members, the members of
/// the modelled errors, and the one member another operation binds. Each is a name the
/// model gave a member somewhere — and a tag key `TagKey` admits, so a caller may send
/// any of them as one.
const MEMBER_NAMES: &[&str] = &[
    "catalog",
    "productId",
    "agreementId",
    "deploymentParameter",
    "tags",
    "expirationDate",
    "clientToken",
    "name",
    "secretString",
    "resourceArn",
    "deploymentParameterId",
    "message",
    "fieldName",
    "resourceId",
    "tagKeys",
];

/// A `PutDeploymentParameter` body carrying the one tag `(key, value)`.
fn body_with_tag(key: &str, value: &str) -> Value {
    json!({
        "agreementId": AGREEMENT,
        "deploymentParameter": { "name": "N", "secretString": LEGAL_SECRET },
        "tags": { key: value },
    })
}

/// The member path the FIRST clause of `reported` states — `… at '<path>' failed to
/// satisfy constraint: <requirement>` — as `(path, requirement)`, or `None` for a
/// sentence stating no clause.
///
/// A sentence states one clause per constraint the request broke, joined by `"; "`, so
/// the first clause is read out of that join rather than out of the whole sentence.
fn clause(reported: &str) -> Option<(String, String)> {
    let first = reported.split("; ").next()?;
    let after = first.split_once(" at '")?.1;
    let (path, rest) = after.split_once('\'')?;
    let requirement = rest.strip_prefix(" failed to satisfy constraint: ")?;
    Some((path.to_string(), requirement.to_string()))
}

/// One broken `TagValue` constraint: the value to send, the requirement the report must
/// state first for it, and how many constraints that value breaks at once.
struct Broken {
    class: &'static str,
    value: String,
    requirement: String,
    clauses: usize,
}

/// Every constraint class `TagValue` states, each with a value breaking it.
///
/// `smithy_query(query="shape",shape="TagsMap")`: `value: TagValue`, `length 1..256`,
/// `pattern ^[a-zA-Z0-9/_+=.:@-]+$` — the bound is the crate's `TAG_VALUE_MAX_LEN`, and
/// the pattern is reported without the anchors the model spells it with.
fn broken_values() -> Vec<Broken> {
    vec![
        Broken {
            class: "one character past the modelled maximum",
            value: "v".repeat(TAG_VALUE_MAX_LEN + 1),
            requirement: format!("Member must have length less than or equal to {TAG_VALUE_MAX_LEN}"),
            clauses: 1,
        },
        Broken {
            class: "far past the modelled maximum",
            value: "v".repeat(TAG_VALUE_MAX_LEN * 4),
            requirement: format!("Member must have length less than or equal to {TAG_VALUE_MAX_LEN}"),
            clauses: 1,
        },
        Broken {
            // The empty value breaks the minimum AND the pattern, which states no
            // repetition of zero characters: two clauses about the same position.
            class: "short of the modelled minimum",
            value: String::new(),
            requirement: "Member must have length greater than or equal to 1".to_string(),
            clauses: 2,
        },
        Broken {
            class: "outside the modelled character class",
            value: "bad value".to_string(),
            requirement: "Member must satisfy regular expression pattern: [a-zA-Z0-9/_+=.:@-]+"
                .to_string(),
            clauses: 1,
        },
    ]
}

#[tokio::test]
async fn a_tag_entry_keyed_like_a_member_name_is_judged_as_the_tag_it_is() {
    let (router, engine) = emulator();
    // A target the account holds, so no row is refused for naming no product or
    // agreement instead of for the tag it carries.
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);

    // Every member name, plus a whole dotted trail spelled as one key (legal: `.` is in
    // `TagKey`'s character class) and a key imitating no member at all as the control.
    let dotted = "deploymentParameter.secretString".to_string();
    let keys: Vec<String> = MEMBER_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .chain([dotted, "owner".to_string()])
        .collect();

    for key in &keys {
        for Broken {
            class,
            value,
            requirement,
            clauses,
        } in broken_values()
        {
            let case = format!("tag key {key:?} with a value {class}");
            let (status, answer) = put(&router, CATALOG, PRODUCT, &body_with_tag(key, &value)).await;
            assert_eq!(
                status, EXPECTED_STATUS,
                "case {case:?}: expected HTTP {EXPECTED_STATUS}, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                EXPECTED_CODE,
                "case {case:?}: expected {EXPECTED_CODE}, got {answer:?}"
            );
            let expected_field = format!("tags.{key}");
            assert_eq!(
                field_name(&answer),
                expected_field,
                "case {case:?}: expected the refusal to name {expected_field:?}, got {answer:?}"
            );
            let reported = message(&answer);
            let expected_envelope = match clauses {
                1 => "1 validation error detected: ".to_string(),
                n => format!("{n} validation errors detected: "),
            };
            assert!(
                reported.starts_with(&expected_envelope),
                "case {case:?}: expected {expected_envelope:?}, got {reported:?}"
            );
            assert_eq!(
                clause(&reported),
                Some((expected_field.clone(), requirement.clone())),
                "case {case:?}: expected the clause to state {expected_field:?} and \
                 {requirement:?}, got {reported:?}"
            );
            // The value IS echoed: a tag value is nothing this model marks `@sensitive`,
            // whatever its key is spelled like. The report caps an echoed value at the
            // same length the model bounds a tag value to, so a value past that maximum
            // is echoed as a prefix of itself and a legal-length one whole.
            let echoed_whole = value.len() <= TAG_VALUE_MAX_LEN;
            let expected_subject = if echoed_whole {
                format!("Value '{value}' at ")
            } else {
                format!("Value '{}", &value[..TAG_VALUE_MAX_LEN])
            };
            assert!(
                reported.contains(&expected_subject),
                "case {case:?}: expected the refused value to be echoed, got {reported:?}"
            );
        }
    }

    // Nothing above was stored: every row was refused before the handler ran.
    let (status, created) = put(
        &router,
        CATALOG,
        PRODUCT,
        &body_with_tag("secretString", "legal"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a tag entry inside every modelled bound must be written, got {status} {created:?}"
    );
    assert_eq!(
        tags_of(&created),
        tag_map(&[("secretString", "legal")]),
        "expected the write to report the tag it stored, got {created:?}"
    );
    // The round trip: the record the write stored is readable back through the
    // implemented read, tag and all.
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "the written record must be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("secretString", "legal")]),
        "expected the stored tag from the record, got {listed:?}"
    );
}

#[tokio::test]
async fn the_modelled_sensitive_member_keeps_its_own_answer() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);

    // The front door still refuses the member the MODEL marks `@sensitive` past its
    // maximum — including in a request that also carries a tag keyed like it, which is
    // no part of the judgement either way.
    let oversized = format!("topsecret-{}", "x".repeat(SECRET_MAX_LEN));
    let rows: Vec<(&str, Value)> = vec![
        (
            "an oversized secret, no tags",
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": oversized },
            }),
        ),
        (
            "an oversized secret beside a tag keyed like it",
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": oversized },
                "tags": { "secretString": "legal" },
            }),
        ),
        (
            "an oversized secret beside a tag keyed like it and over ITS bound",
            json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": { "name": "N", "secretString": oversized },
                "tags": { "secretString": "v".repeat(TAG_VALUE_MAX_LEN + 1) },
            }),
        ),
    ];

    for (case, request_body) in rows {
        let (status, answer) = put(&router, CATALOG, PRODUCT, &request_body).await;
        assert_eq!(
            status, 403,
            "case {case:?}: an oversized sensitive payload must be refused 403, \
             got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ForbiddenException",
            "case {case:?}: expected ForbiddenException, got {answer:?}"
        );
        assert!(
            !answer.to_string().contains("topsecret-"),
            "case {case:?}: a refused secret must not be echoed, got {answer:?}"
        );
    }

    // And the modelled refusal of that same member still names it WITHOUT its value,
    // whatever a tag of the same request is keyed like.
    let (status, answer) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "N", "secretString": "" },
            "tags": { "secretString": "legal" },
        }),
    )
    .await;
    assert_eq!(
        status, EXPECTED_STATUS,
        "a secret short of its minimum must be the modelled 400, got {status} {answer:?}"
    );
    assert_eq!(
        field_name(&answer),
        "deploymentParameter.secretString",
        "expected the refusal to name the modelled member, got {answer:?}"
    );
    let reported = message(&answer);
    assert_eq!(
        clause(&reported),
        Some((
            "deploymentParameter.secretString".to_string(),
            "Member must have length greater than or equal to 1".to_string()
        )),
        "expected the clause to state the sensitive member and its bound, got {reported:?}"
    );
    assert!(
        reported.contains("Value at '"),
        "a @sensitive member's value is not echoed, got {reported:?}"
    );
}
