// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TEMPLATES this service states its refusals in.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test, each asserted over a SET of inputs rather than one:
//!
//! * a constraint refusal is stated as `N validation error(s) detected: Value at
//!   '<member path>' failed to satisfy constraint: Member must <requirement>` — the member
//!   path and the requirement come from the request and the model, and the value the
//!   request submitted is never part of the answer, whichever member and whichever
//!   constraint it is;
//! * a `@pattern` requirement states the member's modelled expression with its whole-value
//!   anchors (`^`, `$`) stripped;
//! * a MAP refused for holding more entries than the modelled `TagMap` maximum is stated
//!   in the schema-validation family instead — `[object has too many properties (found <n>
//!   but schema requires at most <max>)]` — parameterized by the entries submitted and
//!   that maximum;
//! * a resource this service cannot resolve is a 404: the contact's own readers and
//!   writers state `<Resource> with arn <arn> not found`, and the tag plane — which takes
//!   the ARN of any taggable resource — states that a resource was not found and reports
//!   the ARN through `resourceId`;
//! * a well-formed activation code that is not the one the outstanding activation token
//!   holds is the modelled 400 stating the mismatch and naming the contact's ARN, which is
//!   a different sentence from the 404 a contact holding no token answers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::ACTIVATION_CODE_LEN;

use crate::email_contacts::{
    activate, create, create_ok, delete, emulator, error_code, get, issued_code, send_code,
    unknown_arn, Emulator,
};
use crate::resource_tag_writes::{tag, untag};
use crate::resource_tags::{create_tagged, list_tags, pair, tag_map, MAX_TAGS};

/// How AWS NAMES the two resources this service's not-founds are about, and the
/// `resourceType` each reports. Transcribed as literal text — the resource wording is
/// exactly what a not-found of this service diverged on, so it may not be re-asserted from
/// the constants the implementation carries.
const CONTACT_TITLE: &str = "Email Contact";
const CONTACT_TYPE: &str = "emailcontact";
const TOKEN_TITLE: &str = "Activation Token";
const TOKEN_TYPE: &str = "activation-token";

/// The modelled maxima and minima the rows below sit one step outside of, from
/// `smithy_query(query="shape",shape="EmailContactName,EmailContactAddress,TagValue,Token,TagMap")`.
const MAX_NAME_LEN: usize = 64;
const MIN_ADDRESS_LEN: usize = 6;
const MAX_ADDRESS_LEN: usize = 254;
const MAX_TAG_VALUE_LEN: usize = 256;

/// The message a refusal of `count` violations states, over `clauses`.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// One clause: the member blamed, and the requirement it broke. Transcribed rather than
/// rendered, so the suite states the wording independently of the code producing it.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The message a not-found states for the `title` resource named by `arn`.
fn not_found_message(title: &str, arn: &str) -> String {
    format!("{title} with arn {arn} not found")
}

/// The `message` of an error body.
fn message(body: &Value) -> &str {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a modelled failure carries a message, got {body:?}"))
}

/// How the offending request reaches the service.
enum Refused {
    /// `CreateEmailContact` with this body.
    Create(Value),
    /// `TagResource` on a freshly created contact, with this tag map.
    Tag(BTreeMap<String, String>),
    /// `UntagResource` on a freshly created contact, naming these keys.
    Untag(Vec<String>),
    /// `ActivateEmailContact` on a freshly created contact, with this code.
    Activate(String),
}

/// One refused request and the message it must be refused with.
struct Case {
    /// What this row varies, named in every assertion.
    what: &'static str,
    request: Refused,
    expected: String,
}

/// A value of `len` characters the pattern of the member it is written to admits, so only
/// the LENGTH bound can refuse it.
fn padding(len: usize) -> String {
    "a".repeat(len)
}

/// Every row is one request refused by a modelled constraint, the rows differing along
/// what the rule generalises over: which member carries the value, which constraint class
/// it broke (a length minimum, a length maximum, a pattern), whether the member is
/// `@sensitive`, whether the value travels in the body, a path label or a query
/// parameter, whether it sits in a map or a list, and how many violations one request
/// carries.
fn refusal_table() -> Vec<Case> {
    let over_long_name = padding(MAX_NAME_LEN + 1);
    let over_long_address = format!("{}@example.com", padding(MAX_ADDRESS_LEN));
    vec![
        Case {
            what: "an address outside its pattern",
            request: Refused::Create(json!({
                "name": "oncall",
                "emailAddress": "not-an-email",
            })),
            expected: envelope(&[clause(
                "emailAddress",
                "Member must satisfy regular expression pattern: (.+)@(.+)",
            )]),
        },
        Case {
            what: "an address one character below its length minimum",
            request: Refused::Create(json!({
                "name": "oncall",
                "emailAddress": "a@b.c",
            })),
            expected: envelope(&[clause(
                "emailAddress",
                &format!("Member must have length greater than or equal to {MIN_ADDRESS_LEN}"),
            )]),
        },
        Case {
            what: "an address past its length maximum",
            request: Refused::Create(json!({
                "name": "oncall",
                "emailAddress": over_long_address,
            })),
            expected: envelope(&[clause(
                "emailAddress",
                &format!("Member must have length less than or equal to {MAX_ADDRESS_LEN}"),
            )]),
        },
        Case {
            what: "a sensitive name one character past its length maximum",
            request: Refused::Create(json!({
                "name": over_long_name,
                "emailAddress": "oncall@example.com",
            })),
            expected: envelope(&[clause(
                "name",
                &format!("Member must have length less than or equal to {MAX_NAME_LEN}"),
            )]),
        },
        Case {
            what: "a sensitive name outside its pattern",
            request: Refused::Create(json!({
                "name": "on call",
                "emailAddress": "oncall@example.com",
            })),
            expected: envelope(&[clause(
                "name",
                r"Member must satisfy regular expression pattern: [\w-.~]+",
            )]),
        },
        Case {
            what: "a tag value past its length maximum, on the create",
            request: Refused::Create(json!({
                "name": "oncall",
                "emailAddress": "oncall@example.com",
                "tags": { "long": padding(MAX_TAG_VALUE_LEN + 1) },
            })),
            expected: envelope(&[clause(
                "tags.long",
                &format!("Member must have length less than or equal to {MAX_TAG_VALUE_LEN}"),
            )]),
        },
        Case {
            what: "a reserved tag key on the tag write",
            request: Refused::Tag(tag_map([pair("aws:owner", "infra")])),
            expected: envelope(&[clause(
                "tags",
                r"Member must satisfy regular expression pattern: (?!aws:).{1,128}",
            )]),
        },
        Case {
            what: "a tag value past its length maximum, on the tag write",
            request: Refused::Tag(tag_map([pair("long", &padding(MAX_TAG_VALUE_LEN + 1))])),
            expected: envelope(&[clause(
                "tags.long",
                &format!("Member must have length less than or equal to {MAX_TAG_VALUE_LEN}"),
            )]),
        },
        Case {
            what: "a reserved key in a tagKeys list, second of three",
            request: Refused::Untag(vec![
                "team".to_string(),
                "aws:owner".to_string(),
                "env".to_string(),
            ]),
            expected: envelope(&[clause(
                "tagKeys.2.member",
                r"Member must satisfy regular expression pattern: (?!aws:).{1,128}",
            )]),
        },
        Case {
            what: "a sensitive activation code shorter than the only length it admits",
            request: Refused::Activate(padding(ACTIVATION_CODE_LEN - 1)),
            // Too short AND outside the expression, which is one clause each.
            expected: envelope(&[
                clause(
                    "code",
                    &format!(
                        "Member must have length greater than or equal to {ACTIVATION_CODE_LEN}"
                    ),
                ),
                clause(
                    "code",
                    "Member must satisfy regular expression pattern: [a-z0-9]{7}",
                ),
            ]),
        },
        Case {
            what: "a sensitive activation code outside its pattern",
            request: Refused::Activate("ABC1234".to_string()),
            expected: envelope(&[clause(
                "code",
                "Member must satisfy regular expression pattern: [a-z0-9]{7}",
            )]),
        },
    ]
}

/// Drive one row and return its status and body.
async fn drive(em: &Emulator, index: usize, request: &Refused) -> (u16, Value) {
    match request {
        Refused::Create(body) => create(em, body).await,
        Refused::Tag(tags) => {
            let arn = create_tagged(em, &format!("contact-{index}"), None).await;
            tag(em, &arn, tags).await
        }
        Refused::Untag(keys) => {
            let arn = create_tagged(em, &format!("contact-{index}"), None).await;
            untag(em, &arn, keys).await
        }
        Refused::Activate(code) => {
            let arn = create_ok(
                em,
                &format!("contact-{index}"),
                &format!("contact-{index}@example.com"),
            )
            .await;
            activate(em, &arn, code).await
        }
    }
}

#[tokio::test]
async fn a_constraint_refusal_names_the_member_and_never_the_value() {
    let table = refusal_table();
    assert!(
        table.len() >= 5,
        "the refusal table must span the class, got {} rows",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        // One account per row: the published per-account email-contact quota bounds how
        // many contacts one account may hold, and these rows are about wording.
        let em = emulator();
        let (status, body) = drive(&em, index, &case.request).await;

        assert_eq!(
            status, 400,
            "case {:?}: a value outside a modelled constraint must be refused, got {status} / \
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
        assert_eq!(
            stated, case.expected,
            "case {:?}: expected {:?}, got {stated:?}",
            case.what, case.expected
        );
        // No part of the answer QUOTES the value the request submitted, which is what the
        // framework's own wording does and this service's does not.
        assert!(
            !body.to_string().contains("Value '"),
            "case {:?}: the answer must quote no submitted value, got {body:?}",
            case.what
        );
    }
}

/// A request carrying TWO offending values states one clause per violation, so the
/// envelope's count is the number of violations and not a constant.
#[tokio::test]
async fn two_offending_values_state_two_clauses() {
    let em = emulator();
    let (status, body) = create(
        &em,
        &json!({
            "name": padding(MAX_NAME_LEN + 1),
            "emailAddress": "a@b.c",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"two offending members\": expected 400, got {status} / {body:?}"
    );
    let stated = message(&body);
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case \"two offending members\": the envelope must count both clauses, got {stated:?}"
    );
    for requirement in [
        &format!("Member must have length less than or equal to {MAX_NAME_LEN}"),
        &format!("Member must have length greater than or equal to {MIN_ADDRESS_LEN}"),
    ] {
        assert!(
            stated.contains(requirement.as_str()),
            "case \"two offending members\": the answer must state {requirement:?}, got \
             {stated:?}"
        );
    }
    assert!(
        !stated.contains("Value '"),
        "case \"two offending members\": neither clause may quote its value, got {stated:?}"
    );
}

/// A tag map of `count` distinct pairs.
fn generated(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| pair(&format!("key-{n:04}"), &format!("value-{n:04}")))
        .collect()
}

/// RULE: a map holding more entries than the modelled `TagMap` maximum is refused with the
/// schema-style finding, parameterized by the entries submitted and that maximum — through
/// every member a tag map travels; a map AT the maximum is accepted.
#[tokio::test]
async fn an_over_quota_tag_map_is_refused_as_too_many_properties() {
    // Rows differ in how far past the maximum the map is, and in which operation carries
    // it: the create that mints the resource, and the write that merges into one.
    for over in [1usize, 2, 50] {
        let submitted = MAX_TAGS + over;
        let expected = format!(
            "[object has too many properties (found {submitted} but schema requires at most \
             {MAX_TAGS})]"
        );

        let em = emulator();
        let (status, body) = create(
            &em,
            &json!({
                "name": "oncall",
                "emailAddress": "oncall@example.com",
                "tags": Value::Object(
                    generated(submitted)
                        .into_iter()
                        .map(|(key, value)| (key, json!(value)))
                        .collect(),
                ),
            }),
        )
        .await;
        let case = format!("CreateEmailContact with {submitted} tags");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} / {body:?}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );

        let arn = create_tagged(&em, "tagged", None).await;
        let (status, body) = tag(&em, &arn, &generated(submitted)).await;
        let case = format!("TagResource with {submitted} tags");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} / {body:?}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains("validation error"),
            "case {case:?}: a map-cardinality rejection is not stated in the constraint \
             envelope, got {stated:?}"
        );
    }

    // The bound itself: a map holding exactly the maximum is accepted, so the rows above
    // are refused by the cardinality rule and not by the request's shape.
    let em = emulator();
    let arn = create_tagged(&em, "at-the-bound", None).await;
    let full = generated(MAX_TAGS);
    let (status, body) = tag(&em, &arn, &full).await;
    assert_eq!(
        status, 204,
        "case \"a tag map at the modelled maximum\": expected the mutation to succeed, got \
         {status} / {body:?}"
    );
}

/// Every operation that resolves an email-contact ARN, so the not-found sentence is
/// asserted wherever a contact is looked up rather than only where it was reported.
async fn resolve(em: &Emulator, operation: &str, arn: &str) -> (u16, Value) {
    match operation {
        "GetEmailContact" => get(em, arn).await,
        "DeleteEmailContact" => delete(em, arn).await,
        "ListTagsForResource" => list_tags(em, arn).await,
        "TagResource" => tag(em, arn, &tag_map([pair("team", "infra")])).await,
        "UntagResource" => untag(em, arn, &["team".to_string()]).await,
        "SendActivationCode" => send_code(em, arn).await,
        "ActivateEmailContact" => activate(em, arn, &"a".repeat(ACTIVATION_CODE_LEN)).await,
        other => panic!("no wire call for {other:?}"),
    }
}

/// The operations that resolve the email CONTACT itself: their refusal names the resource
/// kind they were asked for.
const CONTACT_OPERATIONS: [&str; 4] = [
    "GetEmailContact",
    "DeleteEmailContact",
    "SendActivationCode",
    "ActivateEmailContact",
];

/// The TAG-PLANE operations, which take the ARN of any taggable resource of this service
/// rather than of the contact alone, and so name no resource kind in their refusal.
const TAG_PLANE_OPERATIONS: [&str; 3] =
    ["ListTagsForResource", "TagResource", "UntagResource"];

/// What the tag plane states for a resource it holds no record of: fixed text, naming
/// neither the resource kind nor the ARN, which it reports through `resourceId` instead.
const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// RULE: an email contact this service cannot resolve is a 404 naming the resource, the
/// ARN the request carried and that nothing resolves it — whichever operation resolved the
/// CONTACT, and whether the ARN never named one or named one since deleted; the tag plane,
/// which takes the ARN of any taggable resource, states that a resource was not found and
/// reports the ARN through `resourceId`.
#[tokio::test]
async fn an_unresolvable_contact_is_reported_with_the_not_found_template() {
    for operation in CONTACT_OPERATIONS.iter().chain(&TAG_PLANE_OPERATIONS) {
        // Rows: an ARN this service never minted, and one it minted and then deleted —
        // the two ways a lookup finds nothing.
        let never_minted = unknown_arn('z');
        let em = emulator();
        let deleted = create_ok(&em, "doomed", "doomed@example.com").await;
        assert_eq!(
            delete(&em, &deleted).await.0,
            200,
            "the delete that makes {deleted} unresolvable must succeed"
        );

        for (origin, arn) in [("never created", &never_minted), ("deleted", &deleted)] {
            let case = format!("{operation} on an arn {origin}");
            let (status, body) = resolve(&em, operation, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: an unresolvable contact must be the modelled 404, got {status} \
                 / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body:?}"
            );
            let stated = message(&body);
            let expected = if TAG_PLANE_OPERATIONS.contains(operation) {
                TAGGED_RESOURCE_NOT_FOUND.to_string()
            } else {
                not_found_message(CONTACT_TITLE, arn)
            };
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
            assert_eq!(
                body.get("resourceId").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: resourceId must name the arn the request carried, got {body:?}"
            );
            assert_eq!(
                body.get("resourceType").and_then(Value::as_str),
                Some(CONTACT_TYPE),
                "case {case:?}: resourceType must name the missing resource, got {body:?}"
            );
        }
    }
}

/// RULE: a well-formed code that is not the one the outstanding token holds is the modelled
/// 400 stating the token mismatch and naming the contact's ARN — a different sentence from
/// the 404 a contact holding NO token answers, which names the token resource instead.
#[tokio::test]
async fn a_code_that_disagrees_with_the_stored_token_states_the_mismatch() {
    let em = emulator();

    // Two contacts, so the ARN in the message is shown to come from the request and not
    // from one recorded value; the codes differ in form as well as in value.
    for (index, wrong) in ["0000000", "zzzzzzz", "a1b2c3d"].iter().enumerate() {
        let name = format!("contact-{index}");
        let arn = create_ok(&em, &name, &format!("{name}@example.com")).await;

        // Before any send there is no token to compare against: the other sentence.
        let (status, body) = activate(&em, &arn, wrong).await;
        let case = format!("{name} with no token issued");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the token 404, got {status} / {body:?}"
        );
        let stated = message(&body);
        let expected = not_found_message(TOKEN_TITLE, &arn);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert_eq!(
            body.get("resourceType").and_then(Value::as_str),
            Some(TOKEN_TYPE),
            "case {case:?}: resourceType must name the token resource, got {body:?}"
        );

        assert_eq!(
            send_code(&em, &arn).await.0,
            200,
            "case {case:?}: the send must mint the activation token"
        );
        let issued = issued_code(&em, &arn).expect("the send must persist a token");
        if issued == *wrong {
            continue;
        }

        let (status, body) = activate(&em, &arn, wrong).await;
        let case = format!("{name} with a token outstanding and a disagreeing code");
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled 400, got {status} / {body:?}"
        );
        let stated = message(&body);
        let expected = format!("Input token did not match stored {TOKEN_TITLE} for arn {arn}");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !body.to_string().contains("Value '"),
            "case {case:?}: the answer must quote no submitted value, got {body:?}"
        );
    }
}
