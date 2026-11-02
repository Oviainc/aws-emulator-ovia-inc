// SPDX-License-Identifier: CC0-1.0

//! Which refusal `DeregisterTaskDefinition` answers for a `taskDefinition` that spells no
//! revision, and how the READ path answers the same values.
//!
//! The rule under test: `DeregisterTaskDefinition` acts on ONE revision — its
//! `taskDefinition` member is "The family and revision (family:revision) or full Amazon
//! Resource Name (ARN) of the task definition to deregister. You must specify a revision."
//! — so an identifier from which no revision can be read is refused HTTP 400
//! `InvalidParameterException` with the fixed sentence "Revision is missing", whatever ELSE
//! is wrong with that identifier. The member's other refusals keep their own classes: a
//! value that DOES spell a revision but is no readable reference earns the family-charset
//! `ClientException`, and one that spells a revision this account holds none of earns the
//! not-found `ClientException`.
//!
//! The contrast the same table asserts: `DescribeTaskDefinition`'s member resolves "the
//! latest ACTIVE revision" of a family, so for it a bare family is an ANSWER rather than a
//! refusal and a family part outside the family character set is the family-charset
//! `ClientException` — the read path keeps that sentence.
//!
//! The rows vary the identifier along what the rule generalises over: the FORM (bare
//! family, `family:*`, an ARN with no revision, an ARN of another resource kind, a value
//! that is no reference at all), its alphabet (spaces, punctuation the family set does not
//! admit, a slash, a lone colon), whether the family it names is stored, and whether the
//! revision part is a number. The recorded divergence — a malformed identifier answered
//! with the describe path's class and sentence — is ONE row.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct.

use serde_json::json;

use crate::ecs_wire::{
    ok, refusal, register_definition, service, string, ACCOUNT, SIGNED_REGION,
};

/// The family holding the revisions every row works beside.
const FAMILY: &str = "revisionrulefamily";

/// The fixed sentence a `taskDefinition` that spells no revision earns where the call acts
/// on one.
const REVISION_MISSING: &str = "Revision is missing";

/// The fixed sentence a reference this service cannot read a family out of earns — the
/// READ path's refusal, which `DescribeTaskDefinition` answers for the same values.
const FAMILY_CHARACTERS: &str = "Family contains invalid characters.";

/// The sentence a reference that names a revision this account holds none of earns.
const NOT_FOUND: &str = "Unable to describe task definition.";

/// An ECS ARN in the caller's own account, for the resource path `resource`.
fn ecs_arn(resource: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:{resource}")
}

/// What `DescribeTaskDefinition` answers for the same identifier: `None` for one it
/// resolves to a revision (HTTP 200), otherwise the `(code, message)` it is refused with.
type DescribeAnswer = Option<(&'static str, &'static str)>;

#[tokio::test]
async fn an_identifier_that_spells_no_revision_is_refused_for_the_missing_revision() {
    let router = service();
    register_definition(&router, FAMILY, "setup").await;
    register_definition(&router, FAMILY, "setup").await;

    // Rows: the case, the identifier, and what the READ path answers for it — the two
    // operations must answer differently, which is what the rule is about.
    let rows: Vec<(&str, String, DescribeAnswer)> = vec![
        ("a stored family, bare", FAMILY.to_string(), None),
        (
            "a stored family with the wildcard revision",
            format!("{FAMILY}:*"),
            None,
        ),
        (
            "the ARN of a stored family, carrying no revision",
            ecs_arn(&format!("task-definition/{FAMILY}")),
            None,
        ),
        (
            "a family that was never registered, bare",
            "ghostfamily".to_string(),
            Some(("ClientException", NOT_FOUND)),
        ),
        (
            "a value whose family part is outside the family character set",
            "not a valid @@@ identifier".to_string(),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
        (
            "a value carrying spaces",
            "spaces are not allowed".to_string(),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
        (
            "a value carrying a slash",
            "slash/inside".to_string(),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
        (
            "a revision part that is no number",
            format!("{FAMILY}:latest"),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
        (
            "nothing but a colon",
            ":".to_string(),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
        (
            "the ARN of another resource kind",
            ecs_arn("service/somecluster/web"),
            Some(("ClientException", FAMILY_CHARACTERS)),
        ),
    ];

    for (case, identifier, describe_answer) in &rows {
        let (code, message) = refusal(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": identifier }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException for {identifier:?}, got \
             {code} with {message:?}"
        );
        assert_eq!(
            message, REVISION_MISSING,
            "case {case:?}: expected {REVISION_MISSING:?} for {identifier:?}, got \
             {message:?}"
        );
        // The sentence is fixed text: it carries nothing the request supplied.
        assert!(
            !message.contains(identifier.as_str()),
            "case {case:?}: expected the sentence to carry nothing of {identifier:?}, got \
             {message:?}"
        );

        // The READ path answers the SAME identifier its own way, so the refusal above
        // belongs to the operation that needs a revision and not to the reference.
        let case = format!("{case}, read back");
        match describe_answer {
            None => {
                let body = ok(
                    &router,
                    "DescribeTaskDefinition",
                    json!({ "taskDefinition": identifier }),
                    &case,
                )
                .await;
                assert_eq!(
                    string(&body["taskDefinition"], "family", &case),
                    FAMILY,
                    "case {case:?}: expected the latest ACTIVE revision of the family, got \
                     {body}"
                );
            }
            Some((expected_code, expected_message)) => {
                let (code, message) = refusal(
                    &router,
                    "DescribeTaskDefinition",
                    json!({ "taskDefinition": identifier }),
                    &case,
                )
                .await;
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (*expected_code, *expected_message),
                    "case {case:?}: expected {expected_code}/{expected_message:?} for \
                     {identifier:?}, got {code}/{message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_identifier_that_spells_a_revision_is_not_refused_for_a_missing_one() {
    let router = service();
    register_definition(&router, FAMILY, "setup").await;
    register_definition(&router, FAMILY, "setup").await;

    // An identifier that DOES spell a revision this account holds is deregistered, in both
    // forms the member documents — so the rule above is about the missing revision rather
    // than about refusing the operation.
    let accepted: [(&str, String, i64); 2] = [
        ("family:revision", format!("{FAMILY}:1"), 1),
        (
            "the full ARN of one revision",
            ecs_arn(&format!("task-definition/{FAMILY}:2")),
            2,
        ),
    ];
    for (case, identifier, revision) in &accepted {
        let body = ok(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": identifier }),
            case,
        )
        .await;
        assert_eq!(
            body["taskDefinition"]["revision"].as_i64(),
            Some(*revision),
            "case {case:?}: expected revision {revision} to be deregistered, got {body}"
        );

        // Read it back through the implemented describe: the write reached the store.
        let read = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": format!("{FAMILY}:{revision}") }),
            case,
        )
        .await;
        assert_eq!(
            string(&read["taskDefinition"], "status", case),
            "INACTIVE",
            "case {case:?}: expected the deregistered revision to read INACTIVE, got {read}"
        );
    }

    // And an identifier that spells a revision NOTHING holds is the unknown identifier it
    // is, not a missing revision: the two conditions keep two classes and two sentences.
    let unresolvable: [(&str, String); 3] = [
        ("a revision the family never minted", format!("{FAMILY}:9")),
        (
            "a revision of a family that was never registered",
            "ghostfamily:1".to_string(),
        ),
        (
            "the ARN of a revision nothing holds",
            ecs_arn("task-definition/ghostfamily:4"),
        ),
    ];
    for (case, identifier) in &unresolvable {
        let (code, message) = refusal(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": identifier }),
            case,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ClientException", NOT_FOUND),
            "case {case:?}: expected the not-found refusal for {identifier:?}, got \
             {code}/{message:?}"
        );
    }
}
