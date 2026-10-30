// SPDX-License-Identifier: CC0-1.0

//! The FORM a refusal of a value inside a collection states, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test:
//!
//! **A refusal of a caller value that sits inside a collection is reported at the POSITION
//! that value occupies, stated the way the centralized constraint walk states a violation
//! there: a value that IS an element of a list collapses onto the collection member and
//! carries the element's requirement set nested inside `Member must satisfy constraint:
//! [ …, Member must not be null]`, while a value held by a MEMBER INSIDE a structure
//! element keeps that member's own full path (`<member>.<position>.member.<key>`) and
//! states its requirement flat.** So which of the two forms a caller is shown depends on
//! where the refused value sat, never on which rule refused it.
//!
//! The witness is the AWS-reserved `aws:` tag-key namespace — the one rule of this service
//! that refuses a value inside a collection and cannot be a modelled constraint, since the
//! modelled `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits `:`
//! (`smithy_query(query="shape",shape="Tag")`) — and it is asserted against the WALK
//! itself: every row drives one operation twice at the same position, once with a reserved
//! key (which the handler's rule refuses) and once with a key one character past the
//! modelled `TagKey` maximum (which the centralized walk refuses), and requires the two
//! answers to differ in nothing but the requirement each states.
//!
//! The rows span what the rule generalises over: both collection forms the model gives a
//! caller for supplying tag keys (a list of keys, `UntagResource$tagKeys`; a list of
//! structures, `TagResource$tags` and `CreateBenefitApplication$Tags`), all three
//! operations that take caller tag keys, the 1-based position of the refused entry (first,
//! middle, last), the length of the list it sits in (1 and 3), and the case the reserved
//! prefix is spelled in.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    reserved_tag_key_requirement, VALIDATION_CODE_INVALID_VALUE,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::reserved_tag_keys::{element_requirement, tag_key_path};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The published maximum of the modelled `TagKey` (`length {min: 1, max: 128}`,
/// `smithy_query(query="shape",shape="TagKey")`), so a row can break it from above and have
/// the centralized walk refuse the very position the handler's rule refuses.
const TAG_KEY_MAX_LEN: usize = 128;

/// The requirement the walk states for a key past that maximum.
fn oversized_requirement() -> String {
    format!("Member must have length less than or equal to {TAG_KEY_MAX_LEN}")
}

/// A key one character past the modelled maximum, inside the modelled `TagKey` pattern.
fn oversized_key() -> String {
    "k".repeat(TAG_KEY_MAX_LEN + 1)
}

/// Where the refused key sits inside the input member, which is what decides the form of
/// the refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    /// The key IS an element of the member (`UntagResource$tagKeys`, a list of keys).
    Element,
    /// The key is the `Key` member of a structure element (`tags` / `Tags`, lists of
    /// `Tag`).
    InsideElement,
}

impl Form {
    /// The path a refusal states for a key at 1-based `position` of `member`.
    fn path(self, member: &str, position: usize) -> String {
        match self {
            Self::Element => member.to_string(),
            Self::InsideElement => tag_key_path(member, position),
        }
    }

    /// `requirement` as a refusal at this position must state it.
    fn requirement(self, requirement: &str) -> String {
        match self {
            Self::Element => element_requirement(requirement),
            Self::InsideElement => requirement.to_string(),
        }
    }
}

/// A `Tags` payload of `length` tags whose entry at 1-based `position` carries `key`; every
/// other entry is a key of the caller's own, inside every modelled constraint.
fn tag_list(key: &str, position: usize, length: usize) -> Value {
    Value::Array(
        (1..=length)
            .map(|slot| {
                let entry = if slot == position {
                    key.to_string()
                } else {
                    format!("own-{slot}")
                };
                json!({ "Key": entry, "Value": "v" })
            })
            .collect(),
    )
}

/// A `tagKeys` payload of `length` keys whose entry at 1-based `position` is `key`.
fn key_list(key: &str, position: usize, length: usize) -> Value {
    Value::Array(
        (1..=length)
            .map(|slot| {
                if slot == position {
                    json!(key)
                } else {
                    json!(format!("own-{slot}"))
                }
            })
            .collect(),
    )
}

/// Call `operation` with `body`, require the modelled `ValidationException` (400), and
/// return its `Message` and the whole body.
async fn refusal(harness: &Harness, case: &str, operation: &str, body: &Value) -> (String, Value) {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse the request with 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {case:?}: expected ValidationException from {operation}, got {out:?}"
    );
    let message = message_of(&out).unwrap_or_else(|| {
        panic!("case {case:?}: the refusal carries no modelled Message member: {out:?}")
    });
    (message, out)
}

/// One row: the same position of one input member, refused twice — by the handler's rule
/// and by the centralized walk.
struct Row {
    case: String,
    operation: &'static str,
    /// The member as the refusal's path names it (the wire member in lowerCamel: the
    /// create spells it `Tags`, and it is reported as `tags`).
    path_member: &'static str,
    form: Form,
    /// The 1-based position the refused key sits at.
    position: usize,
    /// The request carrying a RESERVED key at that position.
    by_the_handler: Value,
    /// The request carrying a key past the modelled maximum at the same position.
    by_the_walk: Value,
}

/// A refused tag key is reported at the position it sat at, in the same form the
/// centralized walk reports a modelled violation of that very position.
#[tokio::test]
async fn a_refused_key_is_stated_the_way_the_walk_states_that_position() {
    let harness = Harness::new();
    let benefit = text("subject", &harness.active_benefit().await, "Id");
    let subject = harness
        .create(
            "subject",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-refusal-form-subject",
                "BenefitIdentifier": benefit,
                "Name": "refusal-form",
            }),
        )
        .await;
    let arn = text("subject", &subject, "Arn");
    let oversized = oversized_key();

    // (the reserved key, its 1-based position, the length of the list it sits in): the
    // first entry of a one-entry list, the middle of three, the last of three — and a
    // different case of the reserved prefix on each.
    let placements: [(&str, usize, usize); 3] = [
        ("aws:createdBy", 1, 1),
        ("AWS:cloudformation:stack-name", 2, 3),
        ("Aws:CreatedBy", 3, 3),
    ];

    let mut rows: Vec<Row> = Vec::new();
    for (index, (key, position, length)) in placements.into_iter().enumerate() {
        rows.push(Row {
            case: format!("TagResource: {key:?} at {position} of {length}"),
            operation: "TagResource",
            path_member: "tags",
            form: Form::InsideElement,
            position,
            by_the_handler: json!({
                "resourceArn": arn,
                "tags": tag_list(key, position, length),
            }),
            by_the_walk: json!({
                "resourceArn": arn,
                "tags": tag_list(&oversized, position, length),
            }),
        });
        rows.push(Row {
            case: format!("UntagResource: {key:?} at {position} of {length}"),
            operation: "UntagResource",
            path_member: "tagKeys",
            form: Form::Element,
            position,
            by_the_handler: json!({
                "resourceArn": arn,
                "tagKeys": key_list(key, position, length),
            }),
            by_the_walk: json!({
                "resourceArn": arn,
                "tagKeys": key_list(&oversized, position, length),
            }),
        });
        rows.push(Row {
            case: format!("CreateBenefitApplication: {key:?} at {position} of {length}"),
            operation: "CreateBenefitApplication",
            path_member: "tags",
            form: Form::InsideElement,
            position,
            by_the_handler: json!({
                "Catalog": CATALOG,
                "ClientToken": format!("tok-refusal-form-handler-{index}"),
                "BenefitIdentifier": benefit,
                "Name": "refusal-form",
                "Tags": tag_list(key, position, length),
            }),
            by_the_walk: json!({
                "Catalog": CATALOG,
                "ClientToken": format!("tok-refusal-form-walk-{index}"),
                "BenefitIdentifier": benefit,
                "Name": "refusal-form",
                "Tags": tag_list(&oversized, position, length),
            }),
        });
    }

    // Both collection forms are in the table, so it cannot shrink to one of them.
    for form in [Form::Element, Form::InsideElement] {
        assert!(
            rows.iter().any(|row| row.form == form),
            "case \"table\": expected a row whose refused key sits at {form:?}, got {:?}",
            rows.iter().map(|row| row.form).collect::<Vec<_>>()
        );
    }

    for row in &rows {
        let case = &row.case;
        let path = row.form.path(row.path_member, row.position);

        // What the centralized walk answers for a MODELLED violation of that position.
        let walk_case = format!("{case} (the centralized walk)");
        let (by_the_walk, _) = refusal(&harness, &walk_case, row.operation, &row.by_the_walk).await;
        let expected_walk = one(&clause(&path, &row.form.requirement(&oversized_requirement())));
        assert_eq!(
            by_the_walk, expected_walk,
            "case {walk_case:?}: expected {expected_walk:?}, got {by_the_walk:?}"
        );

        // What the handler's own rule answers for the same position, which must read the
        // same way: the same path, the same nesting, a different requirement.
        let handler_case = format!("{case} (the handler)");
        let (by_the_handler, out) =
            refusal(&harness, &handler_case, row.operation, &row.by_the_handler).await;
        let expected_handler = one(&clause(
            &path,
            &row.form.requirement(&reserved_tag_key_requirement()),
        ));
        assert_eq!(
            by_the_handler, expected_handler,
            "case {handler_case:?}: expected {expected_handler:?}, got {by_the_handler:?}"
        );
        assert_eq!(
            by_the_handler,
            by_the_walk.replace(&oversized_requirement(), &reserved_tag_key_requirement()),
            "case {handler_case:?}: expected the walk's own form for this position \
             ({by_the_walk:?}) with its requirement replaced, got {by_the_handler:?}"
        );
        assert_validation_invariants(&handler_case, &by_the_handler);

        // The one-member field list follows the same path and requirement.
        let field = out["FieldList"]
            .as_array()
            .and_then(|fields| fields.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            field["Name"],
            json!(path),
            "case {handler_case:?}: the FieldList must name {path:?}, got {out:?}"
        );
        assert_eq!(
            field["Message"],
            json!(row.form.requirement(&reserved_tag_key_requirement())),
            "case {handler_case:?}: the FieldList must state the requirement, got {out:?}"
        );
        assert_eq!(
            field["Code"],
            json!(VALIDATION_CODE_INVALID_VALUE),
            "case {handler_case:?}: the FieldList must carry the invalid-value code, \
             got {out:?}"
        );
    }
}

/// Several reserved keys in one member are the ONE refusal the first of them states: the
/// refusal names one position, under the single-violation envelope, as the handler's rule
/// reports the first element it refuses.
#[tokio::test]
async fn several_reserved_keys_are_reported_at_the_first_of_them() {
    let harness = Harness::new();
    let benefit = text("subject", &harness.active_benefit().await, "Id");
    let subject = harness
        .create(
            "subject",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-refusal-form-first",
                "BenefitIdentifier": benefit,
                "Name": "refusal-form",
            }),
        )
        .await;
    let arn = text("subject", &subject, "Arn");

    // (case, the tags list, the 1-based position the refusal must name)
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "two reserved keys, the first at the front",
            json!([
                { "Key": "aws:createdBy", "Value": "v" },
                { "Key": "own", "Value": "v" },
                { "Key": "AWS:lastUpdatedBy", "Value": "v" },
            ]),
            1,
        ),
        (
            "two reserved keys after a key of the caller's own",
            json!([
                { "Key": "own", "Value": "v" },
                { "Key": "Aws:CreatedBy", "Value": "v" },
                { "Key": "aws:cloudformation:stack-name", "Value": "v" },
            ]),
            2,
        ),
        (
            "every key reserved",
            json!([
                { "Key": "aws:a", "Value": "v" },
                { "Key": "aws:b", "Value": "v" },
            ]),
            1,
        ),
    ];

    for (case, tags, position) in rows {
        let (message, _) = refusal(
            &harness,
            case,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": tags }),
        )
        .await;
        let expected = one(&clause(
            &tag_key_path("tags", position),
            &reserved_tag_key_requirement(),
        ));
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_validation_invariants(case, &message);
    }
}
