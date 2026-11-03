// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service WORDS the refusal of a modelled
//! `@pattern` it has to apply by hand.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a modelled `@pattern` this service applies ITSELF is refused in
//! the same words the framework's centralized constraint walk uses for a `@pattern` it
//! can apply — the `N validation error detected:` envelope around
//! `Value '<the submitted value>' at '<the member path>' failed to satisfy constraint:
//! Member must satisfy regular expression pattern: <the expression the model declares>`,
//! with `fieldList` blaming that same path with the requirement half of that clause —
//! whichever member and whichever expression it is.
//!
//! Two modelled patterns reach this service by hand, for reasons that are the
//! framework's rather than the model's: `EmailContactName`'s `[\w-.~]+`, whose character
//! class the walk's regex engine refuses, and `TagKey`'s `^(?!aws:).{1,128}$`, a map-key
//! constraint the walk does not visit and a look-ahead it cannot compile. The table below
//! spans BOTH, and every member each travels through: `CreateEmailContact$name`,
//! `CreateEmailContact$tags`, `TagResource$tags` and `UntagResource$tagKeys`.
//!
//! The expectation is RENDERED by the framework's own renderer rather than transcribed
//! into the assertion, which is the rule itself; the fixed halves of the sentence (the
//! envelope, `at '<path>'`, the expression) are additionally pinned as literal text, so
//! the suite states AWS's wording independently of the renderer that produces it.

use serde_json::{json, Map, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::{
    EMAIL_CONTACT_NAME_PATTERN, MAX_TAG_KEY_LEN, TAG_KEY_PATTERN,
};

use crate::email_contacts::{create, emulator, error_code, Emulator};
use crate::resource_tag_writes::{tag, untag, TAG_MUTATION_SUCCESS_STATUS};
use crate::resource_tags::{create_tagged, pair, tag_map, tags_ok};

/// What the framework's constraint clause puts between the value it echoes and the
/// requirement that value broke.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// The prefix `TagKey`'s leading `(?!aws:)` look-ahead excludes.
const RESERVED_PREFIX: &str = "aws:";

/// How the offending value reaches the service.
enum Request {
    /// `CreateEmailContact` carrying this `name`.
    Name(String),
    /// `CreateEmailContact` carrying a tag map with these keys.
    CreateTags(Vec<String>),
    /// `TagResource` carrying a tag map with these keys.
    TagResourceTags(Vec<String>),
    /// `UntagResource` naming these keys, in this order.
    UntagTagKeys(Vec<String>),
}

/// One by-hand `@pattern` refusal.
struct Case {
    /// What this row varies, named in every assertion.
    what: &'static str,
    request: Request,
    /// The value the clause must echo: the one value of the request outside the pattern.
    value: String,
    /// The path the clause must blame.
    path: String,
    /// The expression the model declares for the member that path names.
    pattern: &'static str,
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// Every row is one request refused by a pattern this service applies by hand, the rows
/// differing along what the rule generalises over: which of the two patterns is broken,
/// which member the value travels through (a map key, a list element, a plain string),
/// which part of the expression the value breaks (the reserved prefix, each side of the
/// 1..128 bound, a line terminator, a character outside the name class), how many
/// offending values the request carries, and WHERE the offending one sits among the legal
/// ones — which is what decides the position a list path names.
fn refusal_table() -> Vec<Case> {
    let over_long = "k".repeat(MAX_TAG_KEY_LEN + 1);
    vec![
        // `TagKey` through a map KEY: the map member is what a refusal blames, because
        // the centralized walk visits no map key and so has no position to name there.
        Case {
            what: "a reserved tag key on TagResource",
            request: Request::TagResourceTags(keys(&["aws:owner"])),
            value: "aws:owner".to_string(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "the reserved prefix alone on TagResource",
            request: Request::TagResourceTags(keys(&[RESERVED_PREFIX])),
            value: RESERVED_PREFIX.to_string(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "the empty tag key on TagResource",
            request: Request::TagResourceTags(keys(&[""])),
            value: String::new(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "a tag key one character past the maximum on TagResource",
            request: Request::TagResourceTags(vec![over_long.clone()]),
            value: over_long.clone(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "a reserved tag key beside two legal ones on TagResource",
            request: Request::TagResourceTags(keys(&["team", "aws:owner", "env"])),
            value: "aws:owner".to_string(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "two refused tag keys on TagResource, the lexicographically first blamed",
            request: Request::TagResourceTags(keys(&["aws:owner", ""])),
            value: String::new(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        // The same pattern, the same path spelling, through the create that mints the
        // resource the map travels with.
        Case {
            what: "a tag key carrying a line terminator on CreateEmailContact",
            request: Request::CreateTags(keys(&["line\u{2028}break"])),
            value: "line\u{2028}break".to_string(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "a reserved tag key beside a legal one on CreateEmailContact",
            request: Request::CreateTags(keys(&["team", "aws:cost"])),
            value: "aws:cost".to_string(),
            path: "tags".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        // `TagKey` through a LIST element: the walk names an element by its 1-based
        // position and the step into the collection, so the path moves with the position.
        Case {
            what: "a reserved element alone on UntagResource",
            request: Request::UntagTagKeys(keys(&["aws:owner"])),
            value: "aws:owner".to_string(),
            path: "tagKeys.1.member".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "a reserved element after a legal one on UntagResource",
            request: Request::UntagTagKeys(keys(&["team", "aws:owner"])),
            value: "aws:owner".to_string(),
            path: "tagKeys.2.member".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "an over-long element third on UntagResource",
            request: Request::UntagTagKeys(vec![
                "team".to_string(),
                "env".to_string(),
                over_long.clone(),
            ]),
            value: over_long,
            path: "tagKeys.3.member".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        Case {
            what: "two refused elements on UntagResource, the first one blamed",
            request: Request::UntagTagKeys(keys(&["aws:owner", "line\nbreak"])),
            value: "aws:owner".to_string(),
            path: "tagKeys.1.member".to_string(),
            pattern: TAG_KEY_PATTERN,
        },
        // `EmailContactName` through a plain string member: the member names itself.
        Case {
            what: "a name carrying a space on CreateEmailContact",
            request: Request::Name("team alpha".to_string()),
            value: "team alpha".to_string(),
            path: "name".to_string(),
            pattern: EMAIL_CONTACT_NAME_PATTERN,
        },
        Case {
            what: "a name carrying a slash on CreateEmailContact",
            request: Request::Name("a/b".to_string()),
            value: "a/b".to_string(),
            path: "name".to_string(),
            pattern: EMAIL_CONTACT_NAME_PATTERN,
        },
        Case {
            what: "a name of non-ascii letters on CreateEmailContact",
            request: Request::Name("équipe".to_string()),
            value: "équipe".to_string(),
            path: "name".to_string(),
            pattern: EMAIL_CONTACT_NAME_PATTERN,
        },
    ]
}

/// The tag map a request carries for `keys`, each pair valued the same way.
fn tag_object(names: &[String]) -> Value {
    let mut tags = Map::new();
    for name in names {
        tags.insert(name.clone(), json!("v"));
    }
    Value::Object(tags)
}

/// The starting tag set of a contact a write-row is driven against, so a refused write
/// can be shown to have persisted nothing.
fn starting_tags() -> std::collections::BTreeMap<String, String> {
    tag_map([pair("keep", "me")])
}

/// Drive one row and return its status and body.
async fn refused(em: &Emulator, index: usize, request: &Request) -> (u16, Value) {
    match request {
        Request::Name(name) => {
            create(
                em,
                &json!({
                    "name": name,
                    "emailAddress": format!("contact-{index}@example.com"),
                }),
            )
            .await
        }
        Request::CreateTags(names) => {
            create(
                em,
                &json!({
                    "name": format!("contact-{index}"),
                    "emailAddress": format!("contact-{index}@example.com"),
                    "tags": tag_object(names),
                }),
            )
            .await
        }
        Request::TagResourceTags(names) => {
            let arn = create_tagged(em, &format!("contact-{index}"), Some(&starting_tags())).await;
            let request = tag_map(names.iter().map(|name| pair(name, "v")));
            tag(em, &arn, &request).await
        }
        Request::UntagTagKeys(names) => {
            let arn = create_tagged(em, &format!("contact-{index}"), Some(&starting_tags())).await;
            untag(em, &arn, names).await
        }
    }
}

/// The words the framework's own renderer gives a `@pattern` violation at `path`: the
/// whole envelope, and the requirement half of its clause.
///
/// Rendered rather than transcribed, because "the same words the centralized walk uses"
/// IS the rule under test.
fn rendered(path: &str, pattern: &str, value: &str) -> (String, String) {
    let violation = aws_smithy_schema_server::Violation {
        path: path.to_string(),
        kind: aws_smithy_schema_server::ViolationKind::Pattern(pattern.to_string()),
        value: Some(value.to_string()),
        sensitive: false,
    };
    let clause = violation.message();
    let requirement = clause
        .split_once(CONSTRAINT_SEPARATOR)
        .map(|(_, requirement)| requirement.to_string())
        .unwrap_or_else(|| panic!("the rendered clause states a requirement, got {clause:?}"));
    let envelope = aws_smithy_schema_server::envelope(std::slice::from_ref(&violation))
        .unwrap_or_else(|| panic!("one violation renders one envelope"));
    (envelope, requirement)
}

/// A value short enough to name in an assertion message.
fn shown(value: &str) -> String {
    let head: String = value.chars().take(24).collect();
    if head.chars().count() == value.chars().count() {
        format!("{head:?}")
    } else {
        format!("{head:?}… [{} characters]", value.chars().count())
    }
}

/// The `message` of a `ValidationException` body.
fn message(body: &Value) -> &str {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a ValidationException carries a message, got {body:?}"))
}

#[tokio::test]
async fn every_hand_applied_pattern_is_refused_in_the_frameworks_own_words() {
    let table = refusal_table();
    assert!(
        table.len() >= 5,
        "the refusal table must span the class, got {} rows",
        table.len()
    );

    // One account per row: the published per-account email-contact quota bounds how many
    // contacts one account may HOLD at a time, and these rows are about the wording of a
    // refusal rather than about that quota.
    for (index, case) in table.iter().enumerate() {
        let em = emulator();
        let (status, body) = refused(&em, index, &case.request).await;

        assert_eq!(
            status, 400,
            "case {:?}: a value outside the modelled pattern must be refused, got {status} / \
             {body:?}",
            case.what
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected ValidationException, got {body:?}",
            case.what
        );

        let stated = message(&body);
        let (expected, requirement) = rendered(&case.path, case.pattern, &case.value);
        assert_eq!(
            stated, expected,
            "case {:?}: the refusal must read exactly as the framework's renderer words \
             this violation, got {stated:?}",
            case.what
        );

        // The fixed halves of that sentence, pinned as literal text so the suite states
        // the wording independently of the renderer that produced the line above.
        assert!(
            stated.starts_with("1 validation error detected: "),
            "case {:?}: the refusal must carry the validation envelope, got {stated:?}",
            case.what
        );
        assert!(
            stated.contains(&format!("Value '{}' at '{}'", case.value, case.path)),
            "case {:?}: the refusal must echo {} at the member path {:?}, got {stated:?}",
            case.what,
            shown(&case.value),
            case.path
        );
        assert!(
            stated.contains(&format!(
                "{CONSTRAINT_SEPARATOR}Member must satisfy regular expression pattern: {}",
                case.pattern
            )),
            "case {:?}: the refusal must state the expression the model declares, got \
             {stated:?}",
            case.what
        );

        // `fieldList` blames the same path with the requirement half of that clause.
        let fields = body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: expected a fieldList, got {body:?}", case.what));
        assert_eq!(
            fields.len(),
            1,
            "case {:?}: one refused value blames one field, got {fields:?}",
            case.what
        );
        let field = fields
            .first()
            .unwrap_or_else(|| panic!("case {:?}: expected one field, got {fields:?}", case.what));
        assert_eq!(
            field.get("name").and_then(Value::as_str),
            Some(case.path.as_str()),
            "case {:?}: the blamed field must be the member path, got {field:?}",
            case.what
        );
        assert_eq!(
            field.get("message").and_then(Value::as_str),
            Some(requirement.as_str()),
            "case {:?}: the blamed field must carry the requirement half of the clause, got \
             {field:?}",
            case.what
        );
    }
}

/// The same request with its one offending value dropped is accepted, so the refusals
/// above are the pattern's and not the request's shape.
#[tokio::test]
async fn the_same_request_without_the_offending_value_is_accepted() {
    let em = emulator();
    let arn = create_tagged(&em, "accepted", Some(&starting_tags())).await;
    let legal = tag_map([pair("team", "infra"), pair("env", "prod")]);

    let (status, body) = tag(&em, &arn, &legal).await;
    assert_eq!(
        status, TAG_MUTATION_SUCCESS_STATUS,
        "case \"a tag map every key of which the pattern admits\": expected \
         {TAG_MUTATION_SUCCESS_STATUS}, got {status} / {body:?}"
    );
    let mut expected = starting_tags();
    expected.extend(legal);
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, expected,
        "case \"a tag map every key of which the pattern admits\": the pairs must land, got \
         {reported:?}"
    );

    let (status, body) = untag(&em, &arn, &keys(&["team", "env"])).await;
    assert_eq!(
        status, TAG_MUTATION_SUCCESS_STATUS,
        "case \"a tagKeys list every element of which the pattern admits\": expected \
         {TAG_MUTATION_SUCCESS_STATUS}, got {status} / {body:?}"
    );
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported,
        starting_tags(),
        "case \"a tagKeys list every element of which the pattern admits\": the named keys \
         must be removed, got {reported:?}"
    );
}

/// A refused value is ECHOED into the clause, and `TagKey` declares its 1..128 bound
/// INSIDE the pattern the centralized walk cannot apply — so a key of any size reaches
/// the handler. The echo is therefore bounded: a huge key still earns the refusal, which
/// still states the path and the expression, and shows a PREFIX of what was sent rather
/// than letting one request choose the size of its own error response.
#[tokio::test]
async fn a_huge_refused_tag_key_is_echoed_as_a_bounded_prefix() {
    let em = emulator();
    let arn = create_tagged(&em, "huge", Some(&starting_tags())).await;
    let huge = format!("{RESERVED_PREFIX}{}", "k".repeat(100_000));

    let (status, body) = tag(&em, &arn, &tag_map([pair(&huge, "v")])).await;
    assert_eq!(
        status, 400,
        "case \"a tag key far past any bound\": expected 400, got {status} / {body:?}"
    );
    let stated = message(&body);
    assert!(
        stated.contains(TAG_KEY_PATTERN),
        "case \"a tag key far past any bound\": the refusal must still state the modelled \
         expression, got {stated:?}"
    );
    assert!(
        stated.contains("at 'tags'"),
        "case \"a tag key far past any bound\": the refusal must still name the member path, \
         got {stated:?}"
    );
    assert!(
        stated.contains(&"k".repeat(MAX_TAG_KEY_LEN)),
        "case \"a tag key far past any bound\": the refusal must still show what was refused, \
         got {stated:?}"
    );
    assert!(
        stated.len() < huge.len() / 10,
        "case \"a tag key far past any bound\": a {} byte key must not produce a request-sized \
         message, got {} bytes",
        huge.len(),
        stated.len()
    );
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported,
        starting_tags(),
        "case \"a tag key far past any bound\": a refused write persists nothing, got \
         {reported:?}"
    );
}

/// The expression the tag-key refusals state is the one the MODEL declares for the member
/// they blame, read off the generated input schema rather than transcribed.
#[test]
fn the_applied_tag_key_expression_is_the_models_own() {
    let member = UNTAG_RESOURCE_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.member_name() == Some("tagKeys"))
        .expect("UntagResourceRequest declares a `tagKeys` member");
    let declared = member
        .traits()
        .and_then(|traits| {
            traits.get(&aws_smithy_schema_server::constraints::ElementConstraintsTrait::TRAIT_ID)
        })
        .and_then(|found| {
            found
                .as_any()
                .downcast_ref::<aws_smithy_schema_server::constraints::ElementConstraintsTrait>()
        })
        .and_then(|element| {
            element
                .inner()
                .get(&aws_smithy_schema_server::constraints::PatternTrait::TRAIT_ID)
        })
        .and_then(|found| {
            found
                .as_any()
                .downcast_ref::<aws_smithy_schema_server::constraints::PatternTrait>()
        })
        .map(|pattern| pattern.regex);
    assert_eq!(
        declared,
        Some(TAG_KEY_PATTERN),
        "the expression these refusals state must be the one the model declares for the \
         element of UntagResourceRequest$tagKeys, got {declared:?}"
    );
}
