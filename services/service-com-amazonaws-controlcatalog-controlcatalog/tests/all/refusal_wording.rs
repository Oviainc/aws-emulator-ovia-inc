// SPDX-License-Identifier: CC0-1.0

//! How Control Catalog words a refusal one of its HANDLERS decides.
//!
//! Sibling of `validation_messages`, which covers the refusals the boundary constraint
//! walk reports (the `N validation error(s) detected: ` envelope). The rule here is
//! about the refusals a handler decides for itself, where nothing about the request's
//! shape is wrong: a control ARN the catalog does not publish, and a filter an
//! operation will not accept. Each such class carries ONE fixed sentence — a property
//! of the class, not of the request — so the text is byte-identical whatever ARN,
//! filter member or element count the caller sent, and no part of the request is
//! rendered into it.
//!
//! Both tests are therefore TABLES spanning their class: the not-found rows vary the
//! partition, the ARN form (both forms GetControl documents), the Region segment and
//! the identifier's length and character class; the filter rows vary the operation,
//! the filter member, the number of entries, whether the named resources exist, and
//! whether an entry names a resource at all. Every row asserts the sentence in full,
//! and the two filter members are additionally compared with each other, because the
//! identity of their texts is what says the sentence is fixed rather than built from
//! the request.
//!
//! The one entry from each table the recorded divergence cited is marked below; the
//! others are the rest of the class.

use std::collections::BTreeSet;

use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::control_catalog::{error_code, Emulator};
use crate::validation_messages::message_of;

/// The sentence Control Catalog answers a control ARN that names no published control
/// with, for every such ARN.
const CONTROL_NOT_FOUND: &str =
    "AWS Control Tower cannot find a control with the ControlArn value that you provided.";

/// The sentence Control Catalog answers a filter it will not accept with, for every
/// such filter and on every operation that takes one.
const INVALID_FILTER: &str =
    "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.";

/// One `DomainResourceFilter` / `ObjectiveResourceFilter` entry naming `arn`. Both
/// shapes carry a single optional `Arn` member (`smithy_query(query="shape",
/// shape="DomainResourceFilter,ObjectiveResourceFilter")`).
fn entry(arn: &str) -> String {
    format!(r#"{{"Arn":"{arn}"}}"#)
}

/// An entry that names no resource at all: `Arn` is optional in both filter shapes, so
/// this body passes the constraint walk and reaches the handler.
fn empty_entry() -> String {
    "{}".to_string()
}

/// A `ListObjectives` body whose `ObjectiveFilter.Domains` holds `entries`.
fn objectives_body(entries: &[String]) -> String {
    format!(
        r#"{{"ObjectiveFilter":{{"Domains":[{}]}}}}"#,
        entries.join(",")
    )
}

/// A `ListCommonControls` body whose `CommonControlFilter.Objectives` holds `entries`.
fn common_controls_body(entries: &[String]) -> String {
    format!(
        r#"{{"CommonControlFilter":{{"Objectives":[{}]}}}}"#,
        entries.join(",")
    )
}

/// The invariants of the rule, checked on every refusal in both tables: the sentence
/// is exactly the one the class carries, and it renders nothing of the request — no
/// ARN, no member path, and no count (the sentences carry no digit at all).
fn assert_fixed_sentence(case: &str, expected: &str, message: &str) {
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.to_lowercase().contains("arn:"),
        "case {case:?}: the refusal renders an ARN, got {message:?}"
    );
    assert!(
        !message.chars().any(|character| character.is_ascii_digit()),
        "case {case:?}: the refusal renders a count or an identifier, got {message:?}"
    );
}

/// A control ARN that names no published control is refused with the one fixed
/// not-found sentence, in every partition and either documented ARN form.
#[tokio::test]
async fn an_unpublished_control_arn_is_refused_with_the_one_fixed_sentence() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let published = catalog
        .controls
        .values()
        .next()
        .expect("the catalog publishes controls");
    // A published identifier, uppercased: an ARN naming no control while differing
    // from a real one by as little as possible.
    let shouting = published
        .arn
        .rsplit('/')
        .next()
        .expect("a stored control ARN carries an identifier")
        .to_uppercase();

    // (case, ARN). The identifiers vary in length and character class across the
    // `[0-9a-zA-Z_\-]+` the modeled `ControlArn` pattern admits, and the ARN forms are
    // the two GetControl documents.
    let rows: Vec<(String, String)> = vec![
        (
            // The recorded instance: a catalog ARN naming no control.
            "a catalog ARN whose identifier the catalog does not publish".to_string(),
            "arn:aws:controlcatalog:::control/nonexistentctrlid00000000".to_string(),
        ),
        (
            // 34 characters: `ControlArn`'s `@length` minimum, so the shortest ARN the
            // handler can ever be reached with.
            "the shortest ARN the modeled length admits".to_string(),
            "arn:aws:controlcatalog:::control/z".to_string(),
        ),
        (
            "an identifier 1000 characters long".to_string(),
            format!("arn:aws:controlcatalog:::control/{}", "z".repeat(1000)),
        ),
        (
            "an identifier of underscores and dashes".to_string(),
            "arn:aws:controlcatalog:::control/NO_SUCH-CONTROL_AT-ALL".to_string(),
        ),
        (
            "a published identifier in the wrong case".to_string(),
            format!("arn:aws:controlcatalog:::control/{shouting}"),
        ),
        (
            "a catalog ARN in the GovCloud partition".to_string(),
            "arn:aws-us-gov:controlcatalog:::control/absentfromthecatalog".to_string(),
        ),
        (
            "a catalog ARN in the China partition".to_string(),
            "arn:aws-cn:controlcatalog:::control/absentfromthecatalog".to_string(),
        ),
        (
            "a Control Tower ARN no control carries".to_string(),
            "arn:aws:controltower:us-east-1::control/AWS-GR_NO_SUCH_CONTROL".to_string(),
        ),
        (
            "a Control Tower ARN with an empty Region segment".to_string(),
            "arn:aws:controltower:::control/AWS-GR_NO_SUCH_CONTROL".to_string(),
        ),
        (
            "a Control Tower ARN in an isolated partition".to_string(),
            "arn:aws-iso-b:controltower:eu-central-1::control/AWS-GR_NO_SUCH_CONTROL".to_string(),
        ),
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for (case, requested) in rows {
        // The row's expectation is well founded only if the catalog really holds no
        // such control: otherwise GetControl would answer 200 and prove nothing.
        assert!(
            catalog.resolve_control(&requested).is_none(),
            "case {case:?}: {requested} names a control the catalog publishes, so it \
             cannot exercise the not-found refusal"
        );
        let (status, response) = emulator
            .post("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {response:?}"
        );
        let message = message_of(&response)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {response:?}"));
        assert_fixed_sentence(&case, CONTROL_NOT_FOUND, &message);
        // The identifier the caller submitted, which a request-built message would
        // carry in some form.
        let identifier = requested
            .rsplit('/')
            .next()
            .expect("each row's ARN carries an identifier");
        assert!(
            !message.contains(identifier),
            "case {case:?}: the refusal echoes the submitted identifier, got {message:?}"
        );
        sentences.insert(message);
    }

    assert_eq!(
        sentences.len(),
        1,
        "the not-found sentence must be the same for every ARN, got {sentences:?}"
    );
}

/// A filter an operation will not accept is refused with the one fixed invalid-filter
/// sentence — the same text for both filter members, whatever they hold.
#[tokio::test]
async fn an_unacceptable_filter_is_refused_with_the_one_fixed_sentence() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    // Real catalog ARNs, so a row is refused for the filter it is and not because the
    // resource it names is unknown (an unknown one is an empty 200, not a refusal).
    let domains: Vec<String> = catalog.domains.keys().take(7).cloned().collect();
    let objectives: Vec<String> = catalog.objectives.keys().take(5).cloned().collect();
    assert!(
        domains.len() >= 3 && objectives.len() >= 3,
        "the catalog must publish several domains and objectives for this table: \
         got {} domains and {} objectives",
        domains.len(),
        objectives.len()
    );
    let absent_domain = format!("arn:aws:controlcatalog:::domain/{}", "z".repeat(25));
    let absent_objective = format!("arn:aws:controlcatalog:::objective/{}", "z".repeat(25));

    // (case, route, body). `ObjectiveFilter.Domains` and
    // `CommonControlFilter.Objectives` each take one ARN at a time (their member
    // documentation), and neither list declares a `@length`, so every row below
    // reaches the handler.
    let rows: Vec<(String, &str, String)> = vec![
        (
            // The recorded instance of the ListObjectives cause.
            "two domains in an ObjectiveFilter".to_string(),
            "/objectives",
            objectives_body(&[entry(&domains[0]), entry(&domains[1])]),
        ),
        (
            "three domains in an ObjectiveFilter".to_string(),
            "/objectives",
            objectives_body(&[entry(&domains[0]), entry(&domains[1]), entry(&domains[2])]),
        ),
        (
            "every published domain in an ObjectiveFilter".to_string(),
            "/objectives",
            objectives_body(&domains.iter().map(|arn| entry(arn)).collect::<Vec<_>>()),
        ),
        (
            "the same domain twice in an ObjectiveFilter".to_string(),
            "/objectives",
            objectives_body(&[entry(&domains[0]), entry(&domains[0])]),
        ),
        (
            "a published and an unpublished domain in an ObjectiveFilter".to_string(),
            "/objectives",
            objectives_body(&[entry(&domains[0]), entry(&absent_domain)]),
        ),
        (
            "an ObjectiveFilter entry naming no domain".to_string(),
            "/objectives",
            objectives_body(&[empty_entry()]),
        ),
        (
            "two ObjectiveFilter entries, neither naming a domain".to_string(),
            "/objectives",
            objectives_body(&[empty_entry(), empty_entry()]),
        ),
        (
            // The recorded instance of the ListCommonControls cause.
            "two objectives in a CommonControlFilter".to_string(),
            "/common-controls",
            common_controls_body(&[entry(&objectives[0]), entry(&objectives[1])]),
        ),
        (
            "every published objective this table takes in a CommonControlFilter".to_string(),
            "/common-controls",
            common_controls_body(&objectives.iter().map(|arn| entry(arn)).collect::<Vec<_>>()),
        ),
        (
            "a published and an unpublished objective in a CommonControlFilter".to_string(),
            "/common-controls",
            common_controls_body(&[entry(&objectives[0]), entry(&absent_objective)]),
        ),
        (
            "a CommonControlFilter entry naming no objective".to_string(),
            "/common-controls",
            common_controls_body(&[empty_entry()]),
        ),
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for (case, path, body) in rows {
        let (status, response) = emulator.post(path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
        );
        let message = message_of(&response)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {response:?}"));
        assert_fixed_sentence(&case, INVALID_FILTER, &message);
        for member in ["Domains", "domains", "Objectives", "objectives"] {
            assert!(
                !message.contains(member),
                "case {case:?}: the refusal names the filter member {member:?}, got \
                 {message:?}"
            );
        }
        sentences.insert(message);
    }

    assert_eq!(
        sentences.len(),
        1,
        "both filter members must be refused with the same sentence, got {sentences:?}"
    );
}
