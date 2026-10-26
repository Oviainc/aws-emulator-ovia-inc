// SPDX-License-Identifier: CC0-1.0

//! How `InvokeGuardrailChecks` refuses an inline check configuration it cannot
//! read, end to end over the router.
//!
//! The operation's INPUT surface is modelled, so a value outside it is refused
//! before the handler runs at all — which is what this suite is about; what the
//! handler then evaluates is asserted in `guardrail_checks_evaluation.rs`. Two
//! rules, each asserted over a SET of requests:
//!
//! 1. **An enum requirement names every value the member admits.** A value
//!    outside a modelled value set is refused `400 ValidationException` whose
//!    clause names the member and lists the WHOLE modelled set — asserted for
//!    every enum of this operation's input.
//! 2. **A value set is listed in the order this service lists it in.** For the two
//!    sets whose order the recordings show, the clause ends with that order, not
//!    with the order the model happens to declare; and the set this service orders
//!    is exactly the modelled set, so a value the model adds or drops cannot go
//!    unreported.
//!
//! Every assertion names its case and the actual value.

use serde_json::json;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ops::types::{
    GUARDRAIL_CHECKS_CONTENT_FILTER_CATEGORY_VALUES, GUARDRAIL_CHECKS_ROLE_VALUES,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;

// The harness every suite of this target shares.
use crate::bedrock_runtime_inference::{emulator, post_json, Call};

/// The URI `InvokeGuardrailChecks` is bound to
/// (`smithy_query(query="operation",operation="InvokeGuardrailChecks")`).
const PATH: &str = "/guardrail-checks/invoke";

/// The `message` member of a modelled refusal.
fn message_of(call: &Call) -> String {
    call.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE 1: the clause of an enum refusal names the member and lists every value
/// the member admits, so a client reading it learns what to send instead.
///
/// The rows are every enum of this operation's input, each with a value outside
/// it; the ORDER the values are listed in is rule 2's subject, not this one's.
#[tokio::test]
async fn an_enum_refusal_names_the_member_and_lists_every_modelled_value() {
    let router = emulator();
    // (case, the request, the member path the clause must name, values the clause
    // must carry). The value sets are the model's own
    // (`smithy_query(query="shape",shape="GuardrailChecksContentFilterCategory,\
    // GuardrailChecksPromptAttackCategory,\
    // GuardrailChecksSensitiveInformationEntityType,GuardrailChecksRole")`); the
    // collection step in a member path is ONE-BASED, which is what the framework's
    // envelope renders.
    let rows: Vec<(&str, serde_json::Value, &str, &[&str])> = vec![
        (
            "a content-filter category outside its enum",
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "checks": { "contentFilter": { "categories": [
                    { "category": "HATE" },
                    { "category": "NOT_A_REAL_CATEGORY" },
                ] } },
            }),
            "checks.contentFilter.categories.2.member.category",
            &["HATE", "INSULTS", "MISCONDUCT", "SEXUAL", "VIOLENCE"],
        ),
        (
            "a prompt-attack category outside its enum",
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "checks": { "promptAttack": { "categories": [{ "category": "SOMETHING_ELSE" }] } },
            }),
            "checks.promptAttack.categories.1.member.category",
            &["JAILBREAK", "PROMPT_INJECTION", "PROMPT_LEAKAGE"],
        ),
        (
            "a sensitive-information entity type outside its enum",
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "checks": { "sensitiveInformation": { "entities": [{ "type": "SHOE_SIZE" }] } },
            }),
            "checks.sensitiveInformation.entities.1.member.type",
            &["EMAIL", "IP_ADDRESS", "US_SOCIAL_SECURITY_NUMBER"],
        ),
        (
            "a message role outside its enum",
            json!({
                "messages": [
                    { "role": "user", "content": [{ "text": "hello" }] },
                    { "role": "moderator", "content": [{ "text": "hello" }] },
                ],
                "checks": { "contentFilter": { "categories": [{ "category": "HATE" }] } },
            }),
            "messages.2.member.role",
            &["user", "system", "assistant"],
        ),
    ];

    for (case, body, member, values) in rows {
        let call = post_json(&router, PATH, &body).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
        let message = message_of(&call);
        assert!(
            message.contains(member),
            "case {case:?}: expected the clause to name {member:?}, got {message:?}"
        );
        for value in values {
            assert!(
                message.contains(value),
                "case {case:?}: expected the value set to carry {value:?}, got {message:?}"
            );
        }
    }
}

/// RULE 2: a value set this service states an order for is listed in THAT order,
/// and the set it orders is exactly the modelled one.
///
/// The orders are recorded behaviour — the content-filter categories from
/// `bedrock-runtime-guardrail-checks-inline#task#2` and the message roles from
/// `bedrock-runtime-invoke-guardrail-checks-error-paths#task#2`, both of which name
/// the member whose value set the clause lists. The membership half is what keeps
/// an order from going stale: a value the model adds or drops fails it.
#[tokio::test]
async fn a_value_set_is_listed_in_the_order_this_service_lists_it_in() {
    let router = emulator();
    // (case, the request, the order the clause must end with, the modelled value
    // set that order must cover)
    let rows: Vec<(&str, serde_json::Value, &[&str], &[&str])> = vec![
        (
            "the content-filter category set",
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "checks": { "contentFilter": { "categories": [{ "category": "NOPE" }] } },
            }),
            &state::CONTENT_FILTER_CATEGORY_ORDER,
            GUARDRAIL_CHECKS_CONTENT_FILTER_CATEGORY_VALUES,
        ),
        (
            "the message-role set",
            json!({
                "messages": [{ "role": "reviewer", "content": [{ "text": "hello" }] }],
                "checks": { "contentFilter": { "categories": [{ "category": "HATE" }] } },
            }),
            &state::MESSAGE_ROLE_ORDER,
            GUARDRAIL_CHECKS_ROLE_VALUES,
        ),
    ];

    for (case, body, ordered, modelled) in rows {
        let call = post_json(&router, PATH, &body).await;
        let listed = format!("[{}]", ordered.join(", "));
        assert!(
            message_of(&call).ends_with(&listed),
            "case {case:?}: expected the clause to end with {listed:?}, got {:?}",
            message_of(&call)
        );

        let mut ordered_set: Vec<&str> = ordered.to_vec();
        ordered_set.sort_unstable();
        let mut modelled_set: Vec<&str> = modelled.to_vec();
        modelled_set.sort_unstable();
        assert_eq!(
            ordered_set, modelled_set,
            "case {case:?}: expected the order to cover exactly the modelled value set \
             {modelled_set:?}, got {ordered_set:?}"
        );
    }
}
