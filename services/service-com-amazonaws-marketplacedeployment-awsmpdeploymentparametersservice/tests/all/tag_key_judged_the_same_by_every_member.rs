// SPDX-License-Identifier: CC0-1.0

//! One judgement for a tag key, whichever member carries it.
//!
//! The rule under test: AWS's tagging limits say what a tag KEY is — a key of
//! 1..128 characters, outside the namespace AWS reserves for its own generated tags
//! — so a request is held to them wherever it names a tag key, whether it means to
//! create, change or remove one. A string outside those limits names no tag key the
//! service can hold, so it is the modelled `ValidationException` (HTTP 400) naming
//! the member that carried it, with the same sentence through either member; a
//! string inside them is a tag key through either member, so what one stores the
//! other removes.
//!
//! The members this covers here (`smithy_query(query="operation",
//! operation="TagResource,UntagResource,PutDeploymentParameter",
//! model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")`):
//! `TagResource$tags`, `UntagResource$tagKeys` and `PutDeploymentParameter$tags`.
//! Only the LENGTH half of the judgement is the subject here — the namespace half
//! has its own suite (`reserved_tag_keys`) and is carried along as one row, so the
//! two halves are pinned as one rule rather than two.
//!
//! The character class the model's CONSTRAINED tag map states is a constraint of
//! that map and of the member targeting it, so it is no part of this rule: the rows
//! below drive the two unconstrained members and the `unconstrained_tag_characters`
//! suite owns the contrast.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LEN,
};

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, message, provision, put, tag, tag_map, tags_of,
    untag, CATALOG,
};

/// The agreement, product and parameter the one stored record below is written for.
const AGREEMENT: &str = "agmt-key-judgement";
const PRODUCT: &str = "prod-key-judgement";
/// The tag the record carries, which no refused call may change and every accepted
/// row is left holding.
const BASELINE_KEY: &str = "env";
const BASELINE_VALUE: &str = "dev";

/// Strings no tagging member may take as a tag key, spanning what the rule
/// generalises over: which bound is crossed (the lower one, the upper one, the
/// namespace), by how much, and whether the characters are ASCII or multi-byte —
/// the bound counts CHARACTERS, so a multi-byte row at the maximum is a key and one
/// past it is not.
///
/// `TAG_KEY_MAX_LEN` and `RESERVED_TAG_KEY_PREFIX` are this service's statements of
/// AWS's tagging limits. None of these is the key the report cited except the
/// 129-character ASCII row.
fn not_tag_keys() -> Vec<(&'static str, String)> {
    vec![
        ("an empty key, below the lower bound", String::new()),
        (
            "an ASCII key one character past the maximum",
            "k".repeat(TAG_KEY_MAX_LEN + 1),
        ),
        (
            "an ASCII key far past the maximum",
            "k".repeat(TAG_KEY_MAX_LEN * 4),
        ),
        (
            "a multi-byte key one character past the maximum",
            "é".repeat(TAG_KEY_MAX_LEN + 1),
        ),
        (
            "a key in a non-Latin script past the maximum",
            "所".repeat(TAG_KEY_MAX_LEN + 1),
        ),
        (
            "a key whose last character takes it past the maximum",
            format!("{}é", "k".repeat(TAG_KEY_MAX_LEN)),
        ),
        (
            "a key in the namespace AWS reserves",
            format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
        ),
    ]
}

/// Strings every tagging member takes as a tag key: the shortest one, each kind of
/// character at the maximum exactly (so the rows above are one character past a
/// bound rather than merely long), and ordinary keys in between.
fn tag_keys() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest key", "k".to_string()),
        ("an ordinary key", "CostCenter".to_string()),
        (
            "an ASCII key exactly at the maximum",
            "k".repeat(TAG_KEY_MAX_LEN),
        ),
        (
            "a multi-byte key exactly at the maximum",
            "é".repeat(TAG_KEY_MAX_LEN),
        ),
        (
            "a key in a non-Latin script exactly at the maximum",
            "所".repeat(TAG_KEY_MAX_LEN),
        ),
        (
            "a key one character short of the maximum",
            "k".repeat(TAG_KEY_MAX_LEN - 1),
        ),
        (
            "a key that only resembles the reserved prefix",
            "awsome".to_string(),
        ),
    ]
}

/// One emulator holding one deployment parameter tagged with the baseline, and the
/// ARN minted for it.
async fn emulator_with_one_parameter(
) -> (smithy_server_runtime::MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, created) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "KeyJudgement", "secretString": "s" },
            "tags": { BASELINE_KEY: BASELINE_VALUE },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup Put must succeed, got {status} {created:?}"
    );
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

/// Assert `answer` is the modelled 400 refusal on member `field`, and hand back the
/// sentence it states so the two members' sentences can be compared.
fn refusal_message(case: &str, status: u16, answer: &Value, field: &str) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        field_name(answer),
        field,
        "case {case:?}: expected fieldName {field:?}, got {answer:?}"
    );
    message(answer)
}

#[tokio::test]
async fn a_string_that_is_no_tag_key_is_refused_by_every_member_that_names_keys() {
    let (router, arn) = emulator_with_one_parameter().await;
    let baseline = tag_map(&[(BASELINE_KEY, BASELINE_VALUE)]);

    for (index, (case, key)) in not_tag_keys().into_iter().enumerate() {
        // The creating side: the reference sentence every other member must state.
        let written: BTreeMap<String, String> =
            [(key.clone(), "v".to_string())].into_iter().collect();
        let (status, answer) = tag(&router, &arn, &written).await;
        let expected = refusal_message(case, status, &answer, "tags");
        assert!(
            expected.contains(&key),
            "case {case:?}: the refusal must name the string it refused, got {expected:?}"
        );

        // The removing side: the same string, the same refusal, blamed on the member
        // that carried it. A 204 here would report success for a request that named
        // no tag key the service can hold.
        let (status, answer) = untag(&router, &arn, std::slice::from_ref(&key)).await;
        let stated = refusal_message(case, status, &answer, "tagKeys");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the sentence the creating member states ({expected:?}), \
             got {stated:?}"
        );

        // Beside a key the record DOES hold: the request is refused whole, so the
        // legal key is not removed either.
        let (status, answer) = untag(
            &router,
            &arn,
            &[BASELINE_KEY.to_string(), key.clone()],
        )
        .await;
        let stated = refusal_message(case, status, &answer, "tagKeys");
        assert_eq!(
            stated, expected,
            "case {case:?}: a refused removal beside a legal key must state the same \
             sentence ({expected:?}), got {stated:?}"
        );

        // And the creating side of the CONSTRAINED member states it too: the bound is
        // AWS's tagging limit, not a constraint of one member's own map shape.
        let (status, answer) = put(
            &router,
            CATALOG,
            PRODUCT,
            &json!({
                "agreementId": AGREEMENT,
                "deploymentParameter": {
                    "name": format!("KeyJudgementPut{index}"),
                    "secretString": "s",
                },
                "tags": ([(key.clone(), json!("v"))]
                    .into_iter()
                    .collect::<serde_json::Map<String, Value>>()),
            }),
        )
        .await;
        let stated = refusal_message(case, status, &answer, "tags");
        assert_eq!(
            stated, expected,
            "case {case:?}: the constrained member must state the same sentence \
             ({expected:?}), got {stated:?}"
        );

        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            baseline,
            "case {case:?}: a refused tag write or removal must change nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_string_that_is_a_tag_key_round_trips_through_both_tagging_members() {
    let (router, arn) = emulator_with_one_parameter().await;
    let baseline = tag_map(&[(BASELINE_KEY, BASELINE_VALUE)]);

    for (case, key) in tag_keys() {
        let written: BTreeMap<String, String> =
            [(key.clone(), "v".to_string())].into_iter().collect();
        let (status, answer) = tag(&router, &arn, &written).await;
        assert_eq!(
            status, 204,
            "case {case:?}: a tag key inside the bounds must be accepted, got {status} {answer:?}"
        );

        let mut expected = baseline.clone();
        expected.insert(key.clone(), "v".to_string());
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the stored key reported back, got {listed:?}"
        );

        // The same string is a tag key to the removing member, so it comes off again.
        let (status, answer) = untag(&router, &arn, std::slice::from_ref(&key)).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the same key must be removable, got {status} {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            baseline,
            "case {case:?}: the removal must leave the baseline, got {listed:?}"
        );

        // `@idempotent`: a key the record no longer holds is removed by doing
        // nothing, which is a success and not a judgement on the key.
        let (status, answer) = untag(&router, &arn, std::slice::from_ref(&key)).await;
        assert_eq!(
            status, 204,
            "case {case:?}: removing an absent key must stay a success, got {status} {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            baseline,
            "case {case:?}: the repeat must change nothing, got {listed:?}"
        );
    }
}
