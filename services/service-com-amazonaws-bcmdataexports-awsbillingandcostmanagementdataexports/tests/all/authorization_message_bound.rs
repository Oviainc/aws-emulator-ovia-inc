// SPDX-License-Identifier: CC0-1.0

//! The modelled bound of the 403 refusal on EVERY operation of this service that
//! authorizes, over the awsJson1_1 wire.
//!
//! RULE: a refusal this service words states what the caller must act on and carries every
//! string it reports inside that member's OWN modelled bound, whatever the request it
//! answers — so the `Message` of `AccessDeniedException`, a `GenericString` of
//! `@length(0, 1024)` (`smithy_query(query="shape",shape="AccessDeniedException")`), stays
//! inside 1024 characters even where the sentence echoes a resource the model admits twice
//! as many characters of: a `ResourceArn` is an `Arn` of `@length(20, 2048)`
//! (`smithy_query(query="shape",shape="Arn")`). What gives way is the ECHOED resource,
//! which is the caller's own value; the clause around it — the principal, the action and
//! why the decision went the way it did — is what tells the caller which grant to add, so
//! it survives an outsized ARN whole.
//!
//! The operations whose model declares that error are `CreateExport`, `TagResource`,
//! `UntagResource` and `ListTagsForResource` (`smithy_query`: the only ones whose error
//! list carries `AccessDeniedException`). This suite drives the three that are not
//! `UntagResource`, which `tests/all/error_member_bounds.rs` drives over the same lengths;
//! together they cover every operation that reaches the one authorization helper. The
//! refusal classes this service words as a `ValidationException` are covered by
//! `tests/all/refusal_message_bounds.rs` and `tests/all/refusal_message_bound.rs`.
//!
//! The decision is the real one: the principals are made with IAM's and STS's OWN
//! operations over the same engine through [`crate::front_door_authorization::Emulator`],
//! so the documents this service reads are the ones those services wrote.
//!
//! The rows differ along what the rule generalises over: how long the echoed ARN is (its
//! published ceiling, one character past what a message admits, exactly at it, one below
//! it, an ordinary one, and the shortest this service's ARN pattern can spell), which
//! operation is refused, which kind of principal asked (an assumed-role session, an IAM
//! user signing with a long-term key), and which reason clause the refusal carries (no
//! identity-based policy allows the action, versus an explicit deny beside a wildcard
//! allow) — the two clauses differ in length, so each leaves the echo a different amount of
//! room.

use serde_json::{json, Value};

use crate::export_lifecycle::{error_code, padded_arn, MAX_ARN_CHARS, PINNED_REGION};
use crate::front_door_authorization::{allow_all_but, create_request, Emulator};

/// The `@length` ceiling of a `GenericString`, which `AccessDeniedException$Message`
/// targets (smithy_query shape `AccessDeniedException`, member `Message`: `@length(min: 0,
/// max: 1024)`). Stated from the MODEL, so the expectation is not read from the code under
/// test.
const MESSAGE_MAX_CHARS: usize = 1024;

/// The `@length` ceiling of an export's `Name` (smithy_query shape `Export`, member
/// `Name`: `@length(min: 1, max: 128)`, pattern `^[0-9A-Za-z\-_]+$`) — the caller-supplied
/// value the ARN `CreateExport` is judged on is built from.
const MAX_EXPORT_NAME_CHARS: usize = 128;

/// How much of the echoed resource a refusal must still spell when the ARN is too long to
/// fit whole: the caller has to be able to tell WHICH resource was refused, so the echo is
/// shortened rather than dropped. Well under the room the shortest clause leaves, so this
/// pins "a substantial prefix survives" and not one arithmetic answer.
const ECHOED_PREFIX_CHARS: usize = 512;

/// The resource segment of the ARN this suite asks about (see
/// [`crate::export_lifecycle::padded_arn`]).
const BOUNDED_MESSAGE_RESOURCE: &str = "export/bounded_message";

/// The shortest ARN this service's modelled pattern can spell for this account: the empty
/// region the pattern permits and a one-character resource segment.
fn shortest_arn() -> String {
    format!(
        "arn:aws:bcm-data-exports::{}:a",
        ::engine::emulator_account_id()
    )
}

/// The ARN lengths the table walks, in characters.
fn arn_lengths() -> Vec<usize> {
    vec![
        MAX_ARN_CHARS,
        MESSAGE_MAX_CHARS + 1,
        MESSAGE_MAX_CHARS,
        MESSAGE_MAX_CHARS - 1,
        512,
        shortest_arn().chars().count(),
    ]
}

/// The ARN of `chars` characters the table asks about.
fn arn_of(chars: usize) -> String {
    if chars == shortest_arn().chars().count() {
        shortest_arn()
    } else {
        padded_arn(BOUNDED_MESSAGE_RESOURCE, chars)
    }
}

/// The reason clause a refusal carries, and the action it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Nothing the principal holds allows the action.
    NothingAllows,
    /// A wildcard allow with an explicit deny of this action beside it.
    ExplicitDeny,
}

impl Reason {
    /// The clause the refusal must still state whole, for `action`.
    fn clause(self, action: &str) -> String {
        match self {
            Self::NothingAllows => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => {
                format!("with an explicit deny in a policy for the {action} action")
            }
        }
    }
}

/// Assert one refusal: it is the modelled 403, its `Message` fits the modelled bound, and
/// it still states the principal, the action and the reason, plus as much of `resource` as
/// the bound left room for.
fn check_refusal(case: &str, status: u16, body: &Value, action: &str, reason: Reason, resource: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: a caller with no grant must be refused, got {status} with {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"));
    assert!(
        !message.is_empty() && message.chars().count() <= MESSAGE_MAX_CHARS,
        "case {case:?}: Message must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters: {message:?}",
        message.chars().count()
    );
    let clause = reason.clause(action);
    assert!(
        message.contains("is not authorized to perform")
            && message.contains(action)
            && message.contains(&clause),
        "case {case:?}: the refusal must name the principal, the action and the reason {clause:?}, got {message:?}"
    );
    if message.chars().count() < MESSAGE_MAX_CHARS {
        // A resource the sentence had room for is echoed WHOLE, so the bound costs nothing
        // where nothing had to give way.
        assert!(
            message.contains(resource),
            "case {case:?}: a resource the sentence had room for must be echoed whole, got {message:?}"
        );
    } else {
        let prefix: String = resource.chars().take(ECHOED_PREFIX_CHARS).collect();
        assert!(
            message.contains(&prefix),
            "case {case:?}: a shortened echo must still spell the first {ECHOED_PREFIX_CHARS} characters of the resource, got {message:?}"
        );
    }
}

/// The two tagging operations that take a caller-supplied `ResourceArn` and authorize on
/// it, over every ARN length, principal kind and reason clause.
#[tokio::test]
async fn a_tagging_refusal_echoing_any_admissible_arn_stays_within_the_bound() {
    let emulator = Emulator::new();

    // Each principal is refused for a DIFFERENT reason, so the table covers both clauses
    // the decision can report, on both kinds of principal this service resolves. A
    // principal carrying a wildcard allow with ONE action denied is asked for exactly that
    // action, which is the one its own policy refuses.
    let principals: Vec<(&str, String, Reason, Vec<&str>)> = vec![
        (
            "an assumed-role session with no policy",
            emulator
                .assumed_role_key("bcmde-bounded-message-bare", None)
                .await,
            Reason::NothingAllows,
            vec!["TagResource", "ListTagsForResource"],
        ),
        (
            "an IAM user with no policy",
            emulator.user_key("bcmde-bounded-message-user", None).await,
            Reason::NothingAllows,
            vec!["TagResource", "ListTagsForResource"],
        ),
        (
            "an assumed-role session explicitly denied the tagging write",
            emulator
                .assumed_role_key(
                    "bcmde-bounded-message-denied-tag",
                    Some(&allow_all_but("bcm-data-exports:TagResource")),
                )
                .await,
            Reason::ExplicitDeny,
            vec!["TagResource"],
        ),
        (
            "an IAM user explicitly denied the tagging read",
            emulator
                .user_key(
                    "bcmde-bounded-message-denied-read",
                    Some(&allow_all_but("bcm-data-exports:ListTagsForResource")),
                )
                .await,
            Reason::ExplicitDeny,
            vec!["ListTagsForResource"],
        ),
    ];

    for (principal, access_key_id, reason, operations) in &principals {
        for chars in arn_lengths() {
            let arn = arn_of(chars);
            for operation in operations {
                let request = if *operation == "TagResource" {
                    json!({
                        "ResourceArn": arn,
                        "ResourceTags": [{ "Key": "team", "Value": "billing" }],
                    })
                } else {
                    json!({ "ResourceArn": arn })
                };
                let case =
                    format!("{principal} asking {operation} about an ARN of {chars} characters");
                let (status, body) = emulator.call(access_key_id, operation, &request).await;
                check_refusal(
                    &case,
                    status,
                    &body,
                    &format!("bcm-data-exports:{operation}"),
                    *reason,
                    &arn,
                );
            }
        }
    }
}

/// `CreateExport` authorizes on the resource the decision is really taken against — the
/// account's export resource, `export/*` in this service's pinned region, because the
/// export the call would create does not exist yet — so the echoed resource is the SAME
/// whatever `Export.Name` the request carried, at that name's published ceiling and at the
/// shortest the model admits.
#[tokio::test]
async fn a_create_refusal_names_the_account_export_resource_within_the_bound() {
    let emulator = Emulator::new();
    let bare = emulator
        .assumed_role_key("bcmde-bounded-message-create", None)
        .await;
    // The resource an authorization decision about a create is taken against, spelled from
    // the ARN grammar rather than read out of the code under test.
    let decided_about = ::engine::emulator_arn("bcm-data-exports", PINNED_REGION, "export/*");

    for (label, name) in [
        ("a Name at its published ceiling", "n".repeat(MAX_EXPORT_NAME_CHARS)),
        ("the shortest Name the model admits", "n".to_string()),
        ("an ordinary Name", "bounded_message_export".to_string()),
    ] {
        let (status, body) = emulator
            .call(&bare, "CreateExport", &create_request(&name))
            .await;
        check_refusal(
            label,
            status,
            &body,
            "bcm-data-exports:CreateExport",
            Reason::NothingAllows,
            &decided_about,
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(&format!("export/{name}")),
            "case {label:?}: the decision is about the account's export resource, not about \
             the requested name, got {message:?}"
        );
    }
}
