// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service WORDS a refusal.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * a request refused for the values it carried is answered with the
//!   `N validation error(s) detected:` envelope over one clause per offending member, each
//!   clause naming the member PATH and the requirement it broke and stating NO submitted
//!   value, and a `@pattern` requirement advertising the modelled expression without the
//!   whole-value anchors it is written with;
//! * a MAP carrying more entries than its modelled maximum is a refusal of the request's
//!   shape against the schema rather than of a member's own value, and is stated in that
//!   family's words with the count the request sent and the maximum the schema allows;
//! * the tag plane states a FIXED sentence for a resource it holds no record of, naming
//!   neither the ARN the request asked about nor the kind of resource it looked for; that
//!   ARN is reported through `ResourceNotFoundException$resourceId`.
//!
//! Each expectation is TRANSCRIBED here rather than rendered by the code under test, so
//! the suite states AWS's wording independently of what produces it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::email_contacts::{
    activate, call, create, create_ok, delete, emulator, error_code, label, unknown_arn,
};
use crate::resource_tag_writes::{tag, untag, TAG_MUTATION_SUCCESS_STATUS};
use crate::resource_tags::{create_tagged, list_tags, pair, tag_map, tags_ok, MAX_TAGS};

/// The envelope's separator between its count and the clauses it carries.
const ENVELOPE_SEPARATOR: &str = ": ";

/// What a clause puts between its subject and the requirement broken.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// The expression a refusal of the activation `code` ADVERTISES: the modelled `Token`
/// pattern `^[a-z0-9]{7}$` (`smithy_query(query="shape",shape="Token")`) without its
/// whole-value anchors. Transcribed, so the suite pins the published expression itself.
const PUBLISHED_CODE_PATTERN: &str = "[a-z0-9]{7}";

/// The fixed sentence the tag plane states for a resource it holds no record of.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The modelled `Token` length, both bounds (`len 7..7`), which an activation code of any
/// other size breaks.
const CODE_LEN: usize = 7;

/// One refusal of the activation `code` label: the value submitted, and the requirements
/// its clauses must state, in the order the walk finds them (a length bound before the
/// expression).
struct CodeCase {
    /// What this row varies, named in every assertion.
    what: &'static str,
    code: &'static str,
    requirements: Vec<String>,
}

/// The requirement a `@pattern` clause states for the activation code.
fn code_pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {PUBLISHED_CODE_PATTERN}")
}

/// Every row is one `ActivateEmailContact` call whose `code` the model refuses, the rows
/// differing along what the rule generalises over: WHICH constraint the value breaks (the
/// expression, each side of the 7..7 length, or both at once), which character takes it
/// outside the expression (case, punctuation, a non-ASCII letter), and how many clauses
/// the envelope therefore carries.
fn code_table() -> Vec<CodeCase> {
    let pattern = code_pattern_requirement();
    let too_short = format!("Member must have length greater than or equal to {CODE_LEN}");
    let too_long = format!("Member must have length less than or equal to {CODE_LEN}");
    vec![
        CodeCase {
            what: "an uppercase code of the modelled length",
            code: "ABC1234",
            requirements: vec![pattern.clone()],
        },
        CodeCase {
            what: "a code of the modelled length carrying a hyphen",
            code: "abc-123",
            requirements: vec![pattern.clone()],
        },
        CodeCase {
            what: "a code of the modelled length carrying a dot",
            code: "abc.123",
            requirements: vec![pattern.clone()],
        },
        CodeCase {
            what: "a code of the modelled length of non-ASCII letters",
            code: "\u{e9}\u{e8}\u{e7}\u{e0}\u{f9}\u{ee}\u{f4}",
            requirements: vec![pattern.clone()],
        },
        CodeCase {
            what: "a code one character short of the modelled length",
            code: "abc123",
            requirements: vec![too_short.clone(), pattern.clone()],
        },
        CodeCase {
            what: "a code one character past the modelled length",
            code: "abc12345",
            requirements: vec![too_long, pattern.clone()],
        },
        CodeCase {
            what: "a code far short of the modelled length",
            code: "qqq",
            requirements: vec![too_short, pattern],
        },
    ]
}

/// The whole sentence a refusal blaming `path` for `requirements` states.
fn envelope(path: &str, requirements: &[String]) -> String {
    let clauses: Vec<String> = requirements
        .iter()
        .map(|requirement| format!("Value at '{path}'{CONSTRAINT_SEPARATOR}{requirement}"))
        .collect();
    let count = match clauses.len() {
        1 => "1 validation error detected".to_string(),
        n => format!("{n} validation errors detected"),
    };
    format!("{count}{ENVELOPE_SEPARATOR}{}", clauses.join("; "))
}

/// The `message` of an error body.
fn message(body: &Value) -> &str {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a modelled failure carries a message, got {body:?}"))
}

/// A tag set of `count` distinct pairs.
fn generated(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| pair(&format!("key-{n:04}"), &format!("value-{n:04}")))
        .collect()
}

/// The sentence the schema family states for a map of `found` entries against the
/// modelled maximum `max`.
fn too_many_properties(found: usize, max: usize) -> String {
    format!("[object has too many properties (found {found} but schema requires at most {max})]")
}

#[tokio::test]
async fn every_refusal_of_the_activation_code_names_the_member_and_states_no_value() {
    let table = code_table();
    assert!(
        table.len() >= 5,
        "the table must span the class, got {} rows",
        table.len()
    );

    let em = emulator();
    let arn = create_ok(&em, "oncall", "oncall@example.com").await;

    for case in &table {
        let (status, body) = activate(&em, &arn, case.code).await;
        assert_eq!(
            status, 400,
            "case {:?}: a code outside the modelled constraints must be refused, got {status} / \
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
        let expected = envelope("code", &case.requirements);
        assert_eq!(
            stated, expected,
            "case {:?}: expected {expected:?}, got {stated:?}",
            case.what
        );
        assert!(
            !stated.contains(case.code),
            "case {:?}: the refusal must not state the code it refused, got {stated:?}",
            case.what
        );
    }
}

/// The same rule at another member of another operation, and through a path that names a
/// position: nothing about it is keyed to the activation code.
#[tokio::test]
async fn the_same_wording_refuses_every_other_constrained_member() {
    let em = emulator();

    // (case, the request, the member path blamed, the requirement stated)
    let long_name = "n".repeat(65);
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "a contact name past its modelled length",
            json!({ "name": long_name, "emailAddress": "oncall@example.com" }),
            "name",
            "Member must have length less than or equal to 64".to_string(),
        ),
        (
            "an address short of its modelled length",
            json!({ "name": "oncall", "emailAddress": "a@b" }),
            "emailAddress",
            "Member must have length greater than or equal to 6".to_string(),
        ),
        (
            "an address outside its modelled expression",
            json!({ "name": "oncall", "emailAddress": "not-an-address" }),
            "emailAddress",
            "Member must satisfy regular expression pattern: (.+)@(.+)".to_string(),
        ),
    ];

    for (case, request, path, requirement) in rows {
        let (status, body) = create(&em, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} / {body:?}"
        );
        let stated = message(&body);
        let expected = envelope(path, std::slice::from_ref(&requirement));
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_plane_states_a_fixed_sentence_for_a_resource_it_holds_no_record_of() {
    let em = emulator();

    // A contact created and then deleted: the ARN was real once, and the sentence may not
    // depend on that either.
    let deleted = create_ok(&em, "deleted", "deleted@example.com").await;
    let (status, body) = delete(&em, &deleted).await;
    assert_eq!(
        status, 200,
        "deleting the contact must succeed, got {status} / {body:?}"
    );

    // Rows differ along what the rule generalises over: which ARN names nothing (never
    // created, three different ids, and one since deleted) and which of the three tag
    // operations asks about it.
    let arns = [
        ("an ARN of all zeroes", unknown_arn('0')),
        ("an ARN of all z", unknown_arn('z')),
        ("an ARN of all a", unknown_arn('a')),
        ("the ARN of a deleted contact", deleted.clone()),
    ];

    for (what, arn) in arns {
        let writes = tag_map([pair("team", "infra")]);
        let keys = vec!["team".to_string()];
        let answers = [
            ("TagResource", tag(&em, &arn, &writes).await),
            ("ListTagsForResource", list_tags(&em, &arn).await),
            ("UntagResource", untag(&em, &arn, &keys).await),
        ];

        for (operation, (status, body)) in answers {
            let case = format!("{what} through {operation}");
            assert_eq!(
                status, 404,
                "case {case:?}: a tag call naming no stored resource must be refused, got \
                 {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body:?}"
            );
            let stated = message(&body);
            assert_eq!(
                stated, RESOURCE_NOT_FOUND,
                "case {case:?}: expected {RESOURCE_NOT_FOUND:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(&arn),
                "case {case:?}: the sentence must carry no ARN, got {stated:?}"
            );
            assert_eq!(
                body.get("resourceId").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: the ARN asked about must be reported through resourceId, got \
                 {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_map_past_its_modelled_maximum_is_refused_in_the_schema_familys_words() {
    let em = emulator();
    let arn = create_ok(&em, "tagged", "tagged@example.com").await;

    // Rows differ along how far past the maximum the map reaches, and along which
    // operation's `tags` member it travels through.
    for count in [MAX_TAGS + 1, MAX_TAGS + 7, 2 * MAX_TAGS] {
        let case = format!("a tag map of {count} entries on TagResource");
        let (status, body) = tag(&em, &arn, &generated(count)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a map past the modelled maximum must be refused, got {status} / \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        let stated = message(&body);
        let expected = too_many_properties(count, MAX_TAGS);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The same member of the create that mints the resource the map travels with.
    let count = MAX_TAGS + 1;
    let case = format!("a tag map of {count} entries on CreateEmailContact");
    let (status, body) = create(
        &em,
        &json!({
            "name": "over",
            "emailAddress": "over@example.com",
            "tags": Value::Object(
                generated(count)
                    .into_iter()
                    .map(|(key, value)| (key, json!(value)))
                    .collect(),
            ),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} / {body:?}"
    );
    let stated = message(&body);
    let expected = too_many_properties(count, MAX_TAGS);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );

    // The boundary: a map of exactly the modelled maximum is NOT refused, so the sentence
    // above belongs to the count and not to the size of the request.
    let at_the_bound = generated(MAX_TAGS);
    let (status, body) = tag(&em, &arn, &at_the_bound).await;
    assert_eq!(
        status, TAG_MUTATION_SUCCESS_STATUS,
        "case \"a tag map of exactly the modelled maximum\": expected \
         {TAG_MUTATION_SUCCESS_STATUS}, got {status} / {body:?}"
    );
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, at_the_bound,
        "case \"a tag map of exactly the modelled maximum\": every pair must land, got {} pairs",
        reported.len()
    );
}

/// A refused tag-plane call persists nothing and leaves the resource it named readable,
/// so the wording above is all that the refusal does.
#[tokio::test]
async fn a_refused_tag_call_changes_nothing() {
    let em = emulator();
    let start = tag_map([pair("keep", "me")]);
    let arn = create_tagged(&em, "kept", Some(&start)).await;

    let (status, _) = tag(&em, &arn, &generated(MAX_TAGS + 1)).await;
    assert_eq!(
        status, 400,
        "case \"a refused over-full map\": expected 400, got {status}"
    );
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, start,
        "case \"a refused over-full map\": the stored set must be untouched, got {reported:?}"
    );

    let unknown = unknown_arn('q');
    let (status, _) = tag(&em, &unknown, &tag_map([pair("team", "infra")])).await;
    assert_eq!(
        status, 404,
        "case \"a tag write naming nothing\": expected 404, got {status}"
    );
    let (status, body) = list_tags(&em, &unknown).await;
    assert_eq!(
        status, 404,
        "case \"a tag write naming nothing\": nothing may have been created by it, got \
         {status} / {body:?}"
    );
}

/// Every refusal of this service names its shape where a restJson1 client reads it: in
/// the error body's `__type`, and in the error-type response header beside it.
#[tokio::test]
async fn every_refusal_names_its_shape() {
    let em = emulator();
    let arn = create_ok(&em, "shape", "shape@example.com").await;

    // The header a restJson1 client may resolve the shape from carries the shape NAME,
    // for a failure a handler returned as much as for a refusal rendered before one ran.
    let raw = crate::email_contacts::call_http(
        &em,
        "GET",
        &format!("/tags/{}", label(&unknown_arn('0'))),
        None,
    )
    .await;
    let header = raw
        .headers()
        .get("x-amzn-errortype")
        .map(|value| value.to_str().unwrap_or("<not utf-8>").to_string());
    assert_eq!(
        header.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a modelled failure a handler returned\": the error-type header must name the \
         shape, got {header:?}"
    );

    // (case, the answer, the shape it must name)
    let answers = [
        (
            "a modelled failure a handler returned",
            call(
                &em,
                "GET",
                &format!("/tags/{}", label(&unknown_arn('0'))),
                None,
            )
            .await,
            "ResourceNotFoundException",
        ),
        (
            "a refusal of the values the request carried",
            activate(&em, &arn, "ABC1234").await,
            "ValidationException",
        ),
    ];

    for (case, (status, body), shape) in answers {
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a client failure, got {status} / {body:?}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some(shape),
            "case {case:?}: the body must name the shape, got {body:?}"
        );
    }
}
