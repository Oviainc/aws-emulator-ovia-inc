// SPDX-License-Identifier: CC0-1.0

//! The refusal IAM states a value it cannot read as an ARN at all with, end to end over
//! the awsQuery wire.
//!
//! The rule under test: a value supplied for a member whose documentation says it names an
//! ARN, when nothing of that value can be read as an ARN, is refused HTTP 400
//! `ValidationError` whose message states the ARN IAM read BETWEEN two fixed halves —
//! `Invalid ARN:`, one space, that reading, one space, `Could not be parsed!`. Two
//! consequences the rows read back:
//!
//! 1. an unparseable value yields NO reading, so the slot the ARN is stated in renders
//!    EMPTY and the template's two spaces meet: the sentence AWS answers carries two
//!    consecutive spaces after the colon, and the sentence with that whitespace collapsed
//!    (as joining only the non-empty fragments would answer) is a different sentence and
//!    not the one under test;
//! 2. the spaces belong to the template rather than to the value, and the value is quoted
//!    nowhere, so the message is byte-identical for every unparseable value and for every
//!    member that carries one — the member it was supplied for, how it fails to be an ARN
//!    and whether it was supplied at all change nothing about it.
//!
//! The rule is bounded on the other side: a value that IS a well-formed ARN but names a
//! resource the member cannot address has a reading, so it is stated by that class's own
//! sentence, which carries no slot and no doubled space.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: which ARN
//! member carries the value (`NotificationChannel`, `Permissions.PolicyTemplateArn`) and
//! how the value fails to be an ARN (a bare resource name, a missing field, an empty
//! partition, an empty vendor, an empty resource, a first field that is not literally
//! `arn`, leading text before it, and no value at all).
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::delegation_request_filing::held;
use crate::user_lifecycle::{call, element, error_code, router_with_engine};

/// The two fixed halves of AWS's sentence for a value it cannot read as an ARN, written
/// out here rather than read from the implementation's constant: what this suite pins is
/// the SENTENCE AWS answers, so a reworded constant has to fail here rather than agree
/// with itself.
const BEFORE_THE_ARN: &str = "Invalid ARN:";
const AFTER_THE_ARN: &str = "Could not be parsed!";

/// What a value nothing can be read as an ARN from leaves in the slot the sentence states
/// the ARN in: nothing at all.
const NO_READING: &str = "";

/// AWS's sentence with `reading` in the slot it states the ARN in, one space on either
/// side of it — the whole message a refused value is answered with.
fn unparseable_arn_message(reading: &str) -> String {
    format!("{BEFORE_THE_ARN} {reading} {AFTER_THE_ARN}")
}

/// AWS's sentence for a value that IS a well-formed ARN but names a resource the member
/// cannot address — the wording measured for the sibling read of an `EntityArn` of the
/// wrong resource type (`tests/all/human_readable_summaries.rs`), which states no ARN and
/// so carries no slot.
const WRONG_RESOURCE_IN_ARN: &str = "Invalid resource type in ARN";

/// The wire code the query protocol names these refusals with: a client branches on the
/// code, so the sentence is not the only thing a row pins.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// A notification channel every documented rule accepts, so a row varying the template
/// ARN is refused for the TEMPLATE rather than for the channel.
const ACCEPTED_CHANNEL: &str = "arn:aws:sns:us-east-1:123456789012:partner-updates";

/// A policy template ARN every documented rule accepts, so a row varying the channel is
/// refused for the CHANNEL rather than for the template.
const ACCEPTED_TEMPLATE: &str = "arn:aws:iam::aws:policy-template/PartnerDropReadWrite";

/// One `CreateDelegationRequest` carrying the row's channel and template, under its own
/// workflow id. `None` leaves `Permissions.PolicyTemplateArn` off the request entirely,
/// which is a permission that supplies no ARN to read at all — `Permissions` itself is a
/// required member, so that row carries the permission's other member (one policy
/// parameter) in its place.
async fn create(
    router: &MultiServiceRouter,
    workflow: &str,
    channel: &str,
    template: Option<&str>,
) -> (u16, String) {
    let mut params = vec![
        ("Description", "temporary access for a partner engagement"),
        ("NotificationChannel", channel),
        ("RequestorWorkflowId", workflow),
        // Inside `sessionDurationType`'s published range (300..43200).
        ("SessionDuration", "3600"),
        ("OnlySendByOwner", "false"),
    ];
    match template {
        Some(template) => params.push(("Permissions.PolicyTemplateArn", template)),
        None => params.extend([
            ("Permissions.Parameters.member.1.Name", "BucketName"),
            ("Permissions.Parameters.member.1.Values.member.1", "drop"),
            ("Permissions.Parameters.member.1.Type", "string"),
        ]),
    }
    call(router, "CreateDelegationRequest", &params).await
}

/// RULE (consequence 1): a value nothing can be read as an ARN from is refused with the
/// sentence rendered around an EMPTY slot, so the two spaces the template puts on either
/// side of the ARN meet — not with that whitespace collapsed.
#[tokio::test]
async fn a_value_that_is_no_arn_is_refused_with_the_arn_slot_rendered_empty() {
    let (router, engine) = router_with_engine();

    let expected = unparseable_arn_message(NO_READING);
    // What joining only the non-empty fragments of the template — or normalising its
    // whitespace — would answer instead: one space where AWS answers two.
    let collapsed = format!("{BEFORE_THE_ARN} {AFTER_THE_ARN}");

    // Rows: which ARN member carries the value, and how the value fails to be an ARN. A
    // channel stays inside `notificationChannelType`'s own bounds (length 2..400, pattern
    // `^[a-zA-Z0-9:_.-]+$`) and a template inside `arnType`'s (length 20..2048), so each
    // row reaches the refusal under test rather than a modelled constraint.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "a channel that is a bare topic name",
            "partner-updates",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel ARN missing its resource field",
            "arn:aws:sns:us-east-1:123456789012",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel ARN whose resource field is empty",
            "arn:aws:sns:us-east-1:123456789012:",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel ARN whose partition field is empty",
            "arn::sns:us-east-1:123456789012:partner-updates",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel ARN whose vendor field is empty",
            "arn:aws::us-east-1:123456789012:partner-updates",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel whose first field is not literally arn",
            "ARN:aws:sns:us-east-1:123456789012:partner-updates",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a template that is a bare resource name",
            ACCEPTED_CHANNEL,
            Some("policy-template/PartnerDropReadWrite"),
        ),
        (
            "a template ARN whose resource field is empty",
            ACCEPTED_CHANNEL,
            Some("arn:aws:iam::123456789012:"),
        ),
        (
            "a template ARN whose partition field is empty",
            ACCEPTED_CHANNEL,
            Some("arn::iam::123456789012:policy-template/PartnerDrop"),
        ),
        (
            "a template ARN whose vendor field is empty",
            ACCEPTED_CHANNEL,
            Some("arn:aws::us-east-1:123456789012:policy-template/Drop"),
        ),
        (
            "a template ARN with text before its first field",
            ACCEPTED_CHANNEL,
            Some(" arn:aws:iam::aws:policy-template/PartnerDrop"),
        ),
        (
            "a permission supplying no template ARN at all",
            ACCEPTED_CHANNEL,
            None,
        ),
    ];

    for (row, (case, channel, template)) in rows.into_iter().enumerate() {
        let (status, body) = create(
            &router,
            &format!("workflow-unparseable-{row}"),
            channel,
            template,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            VALUE_REFUSAL_CODE,
            "case {case:?}: expected the query protocol's refusal code {VALUE_REFUSAL_CODE:?}, \
             got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_ne!(
            message, collapsed,
            "case {case:?}: expected the empty ARN slot to be rendered between the template's \
             two spaces, got the collapsed sentence {message:?}"
        );
    }

    assert!(
        held(&engine).is_empty(),
        "a refused filing stores nothing, got {:?}",
        held(&engine)
    );
}

/// RULE (consequence 2): the sentence quotes nothing of the value and nothing of the
/// member, so every unparseable value — whatever its length, its shape and the member it
/// was supplied for — is answered the SAME message, byte for byte.
#[tokio::test]
async fn every_unparseable_value_is_answered_the_same_sentence() {
    let (router, engine) = router_with_engine();

    // A template value far past the length of any row above, to show the sentence does not
    // grow with the value it refuses.
    let long_template = "partner-drop-policy-template-".repeat(8);

    // Rows: values differing in length, in alphabet and in how far they get towards being
    // an ARN, split across the two members that carry one.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("the shortest channel the member admits", "no", Some(ACCEPTED_TEMPLATE)),
        (
            "a channel of only the first field",
            "arn",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a channel carrying every field but the vendor",
            "arn:aws-us-gov:us-gov-west-1:210987654321:Partner_Updates.1",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a long template value that is no ARN",
            ACCEPTED_CHANNEL,
            Some(long_template.as_str()),
        ),
        (
            "a template of colons alone",
            ACCEPTED_CHANNEL,
            Some("::::::::::::::::::::"),
        ),
    ];

    let mut answered: Vec<(String, String)> = Vec::new();
    for (row, (case, channel, template)) in rows.into_iter().enumerate() {
        let (status, body) = create(
            &router,
            &format!("workflow-same-sentence-{row}"),
            channel,
            template,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        answered.push((case.to_string(), element(&body, "Message")));
    }

    let expected = unparseable_arn_message(NO_READING);
    for (case, message) in &answered {
        assert_eq!(
            message, &expected,
            "case {case:?}: expected the refusal to carry nothing of the value and to read \
             {expected:?}, got {message:?}"
        );
    }

    assert!(
        held(&engine).is_empty(),
        "a refused filing stores nothing, got {:?}",
        held(&engine)
    );
}

/// RULE (the other side of it): a value that IS a well-formed ARN has a reading, so it is
/// refused by the sentence for the resource it names rather than by the unparseable one —
/// which is how the empty slot stays specific to a value nothing can be read from.
#[tokio::test]
async fn a_well_formed_arn_of_another_resource_keeps_its_own_sentence() {
    let (router, engine) = router_with_engine();

    // Rows: a well-formed ARN of another service, for each member that carries one.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "a channel naming a state machine rather than a topic",
            "arn:aws:states:us-east-1:123456789012:stateMachine:partner-updates",
            Some(ACCEPTED_TEMPLATE),
        ),
        (
            "a template naming a secret rather than an IAM resource",
            ACCEPTED_CHANNEL,
            Some("arn:aws:secretsmanager:us-east-1:123456789012:secret:partner-template"),
        ),
    ];

    for (row, (case, channel, template)) in rows.into_iter().enumerate() {
        let (status, body) = create(
            &router,
            &format!("workflow-wrong-resource-{row}"),
            channel,
            template,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            VALUE_REFUSAL_CODE,
            "case {case:?}: expected the query protocol's refusal code {VALUE_REFUSAL_CODE:?}, \
             got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, WRONG_RESOURCE_IN_ARN,
            "case {case:?}: expected {WRONG_RESOURCE_IN_ARN:?}, got {message:?}"
        );
        assert!(
            !message.contains("  "),
            "case {case:?}: this sentence states no ARN, so it carries no empty slot, got \
             {message:?}"
        );
    }

    assert!(
        held(&engine).is_empty(),
        "a refused filing stores nothing, got {:?}",
        held(&engine)
    );
}
