// SPDX-License-Identifier: CC0-1.0

//! One rule: a list ceiling this crate enforces in a handler is refused with the
//! report `state::constraint_report` builds, whatever the request carried.
//!
//! Two ceilings the model cannot express are decided in handler code — a plan's
//! accumulated tag count (`TagList`'s `@length 0..200` bounds one REQUEST, not the set
//! `TagResource` grows across calls) and `Rescore`'s batch size (`Documents` carries no
//! maximum). Both are refused through the crate's ONE renderer for such a constraint,
//! so the clause names the member the offending list arrived in and the length it had to
//! satisfy, and renders neither the list's contents nor any count derived from the
//! request.
//!
//! The table varies what the rule generalises over: how many keys the request
//! introduces, how the resulting count splits between the keys the plan already holds
//! and the ones the request adds, and the FORM of each new key — its length (up to the
//! published 128-character bound), its character set, and a key shaped like a refusal
//! sentence, which is the adversarial row since a key that LOOKS like a message is
//! still not part of one. Two rows sit at the ceiling rather than past it, so the table
//! pins where the refusal starts.

use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, ViolationKind};

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::scaffolded;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{
    call, create_active_plan, create_plan_with_tags, emulator, error_code, error_message,
};

/// The member a `TagResource` request's tags arrive in, as a validation clause spells
/// the dotted path.
const TAGS_MEMBER: &str = "tags";

/// The member a `Rescore` request's batch arrives in, spelled the same way.
const DOCUMENTS_MEMBER: &str = "documents";

/// A tag with key `key` and an unremarkable value.
fn tag(key: &str) -> Value {
    json!({ "Key": key, "Value": "v" })
}

/// `count` tags under distinct keys prefixed `prefix`.
fn tags(prefix: &str, count: usize) -> Vec<Value> {
    (0..count).map(|n| tag(&format!("{prefix}{n}"))).collect()
}

/// Create a plan named `name` already holding `held` tags, and return its ARN.
///
/// The tags are written through `TagResource` itself, in one request: `TagList` is
/// `length 0..200` (`smithy_query(query="shape",shape="TagList")`), which is the same
/// 200 as the per-plan ceiling, so one call can fill a plan to it.
async fn plan_holding(router: &MultiServiceRouter, name: &str, held: usize) -> String {
    let (_, arn) = create_plan_with_tags(router, name, Value::Null).await;
    if held > 0 {
        let (status, answer) = call(
            router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tags("held", held) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "preparing {name} with {held} tags: got {status} {answer}"
        );
    }
    arn
}

/// One row: a plan holding `held` tags, a request carrying `incoming`, and the status
/// the service answers with.
struct Case {
    case: String,
    held: usize,
    incoming: Vec<Value>,
    expected_status: u16,
}

/// The rows of the tag-ceiling table, over the ceiling the handler enforces.
fn tag_ceiling_cases(ceiling: usize) -> Vec<Case> {
    // The key forms a refusal must not reflect back: the shortest the model admits, one
    // at the published 128-character bound (`TagKey`, `length 1..128`, from
    // `smithy_query(query="shape",shape="TagKey")`), one built from the symbols a tag
    // key may carry ("_ . : / = + - @", `Tag`), a purely numeric one, and one shaped
    // like a refusal sentence.
    let key_forms: Vec<(&str, String)> = vec![
        ("the shortest key the model admits", "k".to_string()),
        (
            "a key at the published 128-character bound",
            "n".repeat(128),
        ),
        (
            "a key built from the symbols a tag key may carry",
            "team.cost:center/eu=1+prod-a@search".to_string(),
        ),
        ("a numeric key", "0123456789".to_string()),
        (
            "a key shaped like a refusal sentence",
            "requested tag count exceeds what the plan may hold".to_string(),
        ),
    ];

    let mut cases: Vec<Case> = key_forms
        .into_iter()
        .map(|(label, key)| Case {
            case: format!("one key past a full set: {label}"),
            held: ceiling,
            incoming: vec![tag(&key)],
            expected_status: scaffolded::CLIENT_ERROR_STATUS,
        })
        .collect();

    // How the resulting count splits between what the plan holds and what the request
    // introduces, and how far past the ceiling it lands.
    cases.push(Case {
        case: "a batch of ten keys against a full set".to_string(),
        held: ceiling,
        incoming: tags("extra", 10),
        expected_status: scaffolded::CLIENT_ERROR_STATUS,
    });
    cases.push(Case {
        case: "a full request against a plan holding one".to_string(),
        held: 1,
        incoming: tags("fresh", ceiling),
        expected_status: scaffolded::CLIENT_ERROR_STATUS,
    });
    cases.push(Case {
        case: "a full request against a plan at half the ceiling".to_string(),
        held: ceiling / 2,
        incoming: tags("fresh", ceiling),
        expected_status: scaffolded::CLIENT_ERROR_STATUS,
    });
    cases.push(Case {
        case: "replacements plus one key too many".to_string(),
        held: ceiling - 5,
        incoming: tags("held", 5)
            .into_iter()
            .chain(tags("fresh", 6))
            .collect(),
        expected_status: scaffolded::CLIENT_ERROR_STATUS,
    });

    // The rows at the ceiling rather than past it: the last key that fits, and a
    // request that only replaces keys the plan already holds (under another spelling of
    // one of them, which is the same key — "Keys are not case sensitive", `Tag$Key`).
    cases.push(Case {
        case: "the last key that fits".to_string(),
        held: ceiling - 1,
        incoming: vec![tag("fresh0")],
        expected_status: 200,
    });
    cases.push(Case {
        case: "a replacement of a key a full set already holds".to_string(),
        held: ceiling,
        incoming: vec![tag("HELD0")],
        expected_status: 200,
    });
    cases
}

#[tokio::test]
async fn a_tag_ceiling_refusal_is_the_one_constraint_report_and_names_nothing_of_the_request() {
    let router = emulator();
    let ceiling = state::MAX_TAGS_PER_PLAN;
    let expected_message = state::constraint_report(
        TAGS_MEMBER,
        ViolationKind::MaxLength(state::MAX_TAGS_PER_PLAN as i64),
    );

    for (index, case) in tag_ceiling_cases(ceiling).into_iter().enumerate() {
        let arn = plan_holding(&router, &format!("ceiling-plan-{index}"), case.held).await;
        let resulting = case.held + case.incoming.len();
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": Value::Array(case.incoming.clone()) }),
        )
        .await;
        assert_eq!(
            status, case.expected_status,
            "case {:?}: expected {}, got {status} {answer}",
            case.case, case.expected_status
        );
        if case.expected_status == 200 {
            continue;
        }
        assert_eq!(
            error_code(&answer),
            state::VALIDATION_EXCEPTION,
            "case {:?}: got {answer}",
            case.case
        );
        let message = error_message(&answer);
        assert_eq!(
            message, expected_message,
            "case {:?}: expected the report the crate's one renderer builds, got {message:?}",
            case.case
        );
        // Nothing the request carried reaches the body: neither a key it named nor the
        // count its keys would have taken the plan to.
        for tag in &case.incoming {
            let key = tag["Key"].as_str().expect("every row's tag carries a key");
            assert!(
                !message.contains(key),
                "case {:?}: the refusal names the request's key {key:?}, got {message:?}",
                case.case
            );
        }
        assert!(
            !message.contains(&resulting.to_string()),
            "case {:?}: the refusal names the request-derived count {resulting}, got {message:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn both_handler_stated_list_ceilings_are_worded_by_the_one_renderer() {
    let router = emulator();

    // The tag ceiling: a plan filled to it, then one key more.
    let arn = plan_holding(&router, "two-ceilings-plan", state::MAX_TAGS_PER_PLAN).await;
    let (status, answer) = call(
        &router,
        "TagResource",
        json!({ "ResourceARN": arn, "Tags": [tag("one-too-many")] }),
    )
    .await;
    assert_eq!(
        status,
        scaffolded::CLIENT_ERROR_STATUS,
        "the tag ceiling: got {status} {answer}"
    );
    let tag_message = error_message(&answer);

    // The batch ceiling: one document past it, every document well formed, so the
    // ceiling is what refuses the request.
    let plan = create_active_plan(&router, "two-ceilings-rescore").await;
    let documents: Vec<Value> = (0..=state::MAX_DOCUMENTS_PER_RESCORE)
        .map(|n| json!({ "Id": format!("d{n}"), "OriginalScore": 0.5, "Body": "a body" }))
        .collect();
    let (status, answer) = call(
        &router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": plan,
            "SearchQuery": "intelligent ranking",
            "Documents": documents,
        }),
    )
    .await;
    assert_eq!(
        status,
        scaffolded::CLIENT_ERROR_STATUS,
        "the batch ceiling: got {status} {answer}"
    );
    let batch_message = error_message(&answer);

    // Each is the report the one renderer builds for its own member and bound ...
    let cases = [
        (
            "the tag ceiling",
            tag_message.clone(),
            TAGS_MEMBER,
            state::MAX_TAGS_PER_PLAN,
        ),
        (
            "the batch ceiling",
            batch_message.clone(),
            DOCUMENTS_MEMBER,
            state::MAX_DOCUMENTS_PER_RESCORE,
        ),
    ];
    for (case, message, member, bound) in &cases {
        let expected = state::constraint_report(member, ViolationKind::MaxLength(*bound as i64));
        assert_eq!(
            &message, &&expected,
            "case {case:?}: expected the report the crate's one renderer builds, got {message:?}"
        );
    }

    // ... so the two sentences differ in nothing but the member and the bound.
    let shapes: Vec<String> = cases
        .iter()
        .map(|(_, message, member, bound)| {
            message
                .replace(member, "<member>")
                .replace(&bound.to_string(), "<bound>")
        })
        .collect();
    assert_eq!(
        shapes[0], shapes[1],
        "the two handler-stated list ceilings must word their refusal identically: \
         {tag_message:?} vs {batch_message:?}"
    );
}
