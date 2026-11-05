// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH RULE a refused redrive configuration reports, driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness
//! in [`super::support`].
//!
//! The rule under test: **a redrive configuration naming an ARN outside the namespace of the
//! queue it is written on is refused `InvalidParameterValue` at HTTP 400, and the `Reason`
//! clause of that refusal states the constraint THAT value broke — the dead-letter target's
//! region rule, the dead-letter target's owner rule, or the source-ARN region rule of the
//! permission document — so no two of this service's redrive rules are reported with one
//! sentence, and the attribute is left exactly as it was.**
//!
//! The `Reason` clause is the only part of an `InvalidParameterValue` that says which
//! constraint a document broke, so a client (or an operator reading a log) that branches on
//! it is sent to the right field only where each rule carries its own sentence.
//!
//! Each table spans what the rule generalises over rather than one example: the HOME REGION
//! the queue lives in (three, on three continents, so nothing can be keyed to one label),
//! the FIELD of the ARN that leaves the namespace (region, account, both at once), the
//! PARAMETER that carried it (`RedrivePolicy`'s `deadLetterTargetArn` and
//! `RedriveAllowPolicy`'s `sourceQueueArns`), the COUNT and the MIX of ARNs a permission
//! lists, and both paths that write a queue attribute (`CreateQueue` and
//! `SetQueueAttributes`).

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    DEAD_LETTER_TARGET_MISSING_REASON, DEAD_LETTER_TARGET_OWNER_REASON,
    DEAD_LETTER_TARGET_REGION_REASON, DEAD_LETTER_TARGET_TYPE_REASON, REDRIVE_ALLOW_POLICY_REASON,
    REDRIVE_ALLOW_POLICY_REGION_REASON, SOURCE_QUEUE_ARNS_REQUIRED_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    account, as_map, call_in, create_in, create_ok_in, emulator, error_code, expected_arn_in,
    ok_in, OTHER_ACCOUNT, REGION,
};

/// The regions a queue of a row lives in: the harness's own plus two more on other
/// continents, so no row can be satisfied by a rule keyed to one region label.
const HOME_REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-2"];

/// The regions a row's far ARNs name, each different from every home region above, so the
/// ARN of one queue name differs from the home one in exactly the region field.
const FAR_REGIONS: &[&str] = &["us-west-2", "sa-east-1"];

/// The attribute carrying the dead-letter configuration of a source queue.
const REDRIVE_POLICY: &str = "RedrivePolicy";

/// The attribute carrying the permission of a dead-letter queue.
const REDRIVE_ALLOW_POLICY: &str = "RedriveAllowPolicy";

/// The error code every refused parameter VALUE of this service carries.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The `Reason` sentence Amazon SQS states for a `deadLetterTargetArn` of another region,
/// spelled out rather than imported, so a row fails if the wording the service answers with
/// drifts. Source: the recorded 400 response of this refusal, whose text is wholly fixed.
const TARGET_REGION_REASON: &str = "Dead-letter target must be in same region as the source.";

/// The `Reason` sentence for a `deadLetterTargetArn` of another account: the other field of
/// the ARN that leaves the queue's namespace, and its own rule.
const TARGET_OWNER_REASON: &str = "Dead-letter target owner should be same as the source.";

/// The `Reason` sentence for a `RedriveAllowPolicy` whose `sourceQueueArns` leave the
/// region: the same boundary seen from the dead-letter queue's end of the link, which Amazon
/// SQS words for the parameter that carried the ARNs.
const ALLOW_POLICY_REGION_REASON: &str =
    "Amazon SQS can't create the redrive allow policy. All source queue ARNs must be in the \
     same region as the current SQS queue.";

/// A fragment of the sentence about which SOURCE queues a dead-letter queue admits — the
/// `allowAll` permission's own rule. No refusal of a `deadLetterTargetArn` may state it: it
/// is a different constraint, on a different queue's attribute.
const ADMISSION_FRAGMENT: &str = "can specify this queue as the dead-letter queue";

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The refused-parameter-value template this service answers a refused VALUE in, spelled out
/// rather than imported: the document the request supplied, the parameter it was supplied
/// for, and the one rule that value broke.
fn refusal(supplied: &str, parameter: &str, reason: &str) -> String {
    format!("Value {supplied} for parameter {parameter} is invalid. Reason: {reason}")
}

/// A `RedrivePolicy` document driving a queue's messages to `target_arn`.
fn redrive_policy(target_arn: &str) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": 3}).to_string()
}

/// A `RedriveAllowPolicy` admitting exactly the source queues `source_arns` names.
fn allow_policy(source_arns: &[String]) -> String {
    json!({"redrivePermission": "byQueue", "sourceQueueArns": source_arns}).to_string()
}

/// One attribute of the queue at `url`, read in `region`, or `None` where the queue carries
/// no value for it.
async fn attribute_in(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    attribute: &str,
) -> Option<String> {
    let reported = ok_in(
        router,
        region,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    as_map(&reported["Attributes"]).get(attribute).cloned()
}

/// Write `attributes` onto the queue at `url` in `region`, answering `(status, body)`.
async fn set_in(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    attributes: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": attributes}),
    )
    .await
}

/// One row of the dead-letter target table: an ARN a `RedrivePolicy` names, and the `Reason`
/// the refusal must state (or `None` for a target the policy may name).
struct TargetRow {
    case: String,
    target_arn: String,
    reason: Option<&'static str>,
}

/// The target rows for a queue living in `home`: the target of this very region, the same
/// queue name in each far region, another account's queue of this region, and another
/// account's queue of a far region.
fn target_rows(home: &str, target: &str) -> Vec<TargetRow> {
    let mut rows = vec![
        TargetRow {
            case: format!("a target of the queue's own region {home}"),
            target_arn: expected_arn_in(home, target),
            reason: None,
        },
        TargetRow {
            case: format!("a target of another account in {home}"),
            target_arn: format!("arn:aws:sqs:{home}:{OTHER_ACCOUNT}:{target}"),
            reason: Some(TARGET_OWNER_REASON),
        },
    ];
    for far in FAR_REGIONS {
        rows.push(TargetRow {
            case: format!("a target of {far} for a queue of {home}"),
            target_arn: expected_arn_in(far, target),
            reason: Some(TARGET_REGION_REASON),
        });
        rows.push(TargetRow {
            case: format!("a target of another account in {far} for a queue of {home}"),
            target_arn: format!("arn:aws:sqs:{far}:{OTHER_ACCOUNT}:{target}"),
            reason: Some(TARGET_OWNER_REASON),
        });
    }
    rows
}

/// A refused `RedrivePolicy` is answered with the rule its own `deadLetterTargetArn` broke —
/// the target's region rule or the target's owner rule, never the sentence about which
/// source queues a dead-letter queue admits — through both paths that write the attribute,
/// and leaves the attribute as it was.
#[tokio::test]
async fn a_refused_dead_letter_target_reports_the_rule_that_arn_broke() {
    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other-account rows: the id must not be this emulator's"
    );
    let target = "reason-dlq";

    for home in HOME_REGIONS {
        for (index, row) in target_rows(home, target).into_iter().enumerate() {
            let TargetRow {
                case,
                target_arn,
                reason,
            } = row;
            let router = emulator();
            // The dead-letter queue really exists in the home region AND in every far
            // region, so a refusal is about the namespace the ARN names and not about a
            // queue nothing created.
            let _ = create_ok_in(&router, home, target, &json!({})).await;
            for far in FAR_REGIONS {
                let _ = create_ok_in(&router, far, target, &json!({})).await;
            }
            let policy = redrive_policy(&target_arn);
            let expected = reason.map(|reason| refusal(&policy, REDRIVE_POLICY, reason));

            // 1. The SetQueueAttributes path, onto a queue carrying no policy at all.
            let source = format!("reason-source-{index}");
            let url = create_ok_in(&router, home, &source, &json!({})).await;
            let (status, parsed) =
                set_in(&router, home, &url, &json!({REDRIVE_POLICY: policy})).await;
            let stored = attribute_in(&router, home, &url, REDRIVE_POLICY).await;
            match &expected {
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case} on SetQueueAttributes: expected the policy stored, got \
                         {status} with {parsed}"
                    );
                    assert_eq!(
                        stored.as_deref(),
                        Some(policy.as_str()),
                        "case {case}: expected the queue to report the policy it took, got \
                         {stored:?}"
                    );
                }
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&parsed).as_str(), message_of(&parsed)),
                        (400, INVALID_PARAMETER_VALUE, message.clone()),
                        "case {case} on SetQueueAttributes: expected the refusal to state its \
                         own rule, got {status} with {parsed}"
                    );
                    assert!(
                        !message_of(&parsed).contains(ADMISSION_FRAGMENT),
                        "case {case}: the refusal must not state the source-admission rule, got \
                         {:?}",
                        message_of(&parsed)
                    );
                    assert_eq!(
                        stored, None,
                        "case {case}: a refused policy must leave the attribute unset, got \
                         {stored:?}"
                    );
                }
            }

            // 2. The CreateQueue path, which judges the same document the same way.
            let created = format!("reason-created-{index}");
            let (status, parsed) =
                create_in(&router, home, &created, &json!({REDRIVE_POLICY: policy})).await;
            match &expected {
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case} on CreateQueue: expected the policy stored, got {status} \
                         with {parsed}"
                    );
                }
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&parsed).as_str(), message_of(&parsed)),
                        (400, INVALID_PARAMETER_VALUE, message.clone()),
                        "case {case} on CreateQueue: expected the refusal to state its own rule, \
                         got {status} with {parsed}"
                    );
                    let (status, parsed) =
                        call_in(&router, home, "GetQueueUrl", &json!({"QueueName": created})).await;
                    assert_eq!(
                        (status, error_code(&parsed).as_str()),
                        (400, "QueueDoesNotExist"),
                        "case {case}: a refused create must leave no queue behind, got {status} \
                         with {parsed}"
                    );
                }
            }
        }
    }
}

/// One row of the permission table: the ARNs a `RedriveAllowPolicy` lists as source queues,
/// and the `Reason` the refusal must state (or `None` for a document the rules admit).
struct AllowRow {
    case: String,
    source_arns: Vec<String>,
    reason: Option<String>,
}

/// The permission rows for a dead-letter queue living in `home`: one source of its own
/// region, one of a far region, a mixture of both, two far ones, and a list naming nothing at
/// all.
fn allow_rows(home: &str) -> Vec<AllowRow> {
    let here = expected_arn_in(home, "allow-source");
    let far_arns: Vec<String> = FAR_REGIONS
        .iter()
        .map(|far| expected_arn_in(far, "allow-source"))
        .collect();
    let mut rows = vec![
        AllowRow {
            case: format!("one source of the queue's own region {home}"),
            source_arns: vec![here.clone()],
            reason: None,
        },
        AllowRow {
            // The permission that admits exactly the queues its list names has to name one,
            // so this document is refused for its ARITY rather than for any ARN's region.
            case: "a list naming no source queue at all".to_string(),
            source_arns: Vec::new(),
            reason: Some(SOURCE_QUEUE_ARNS_REQUIRED_REASON.to_string()),
        },
        AllowRow {
            case: format!("two sources of the queue's own region {home}"),
            source_arns: vec![here.clone(), expected_arn_in(home, "allow-second")],
            reason: None,
        },
        AllowRow {
            case: "every listed source in a far region".to_string(),
            source_arns: far_arns.clone(),
            reason: Some(ALLOW_POLICY_REGION_REASON.to_string()),
        },
    ];
    for far in FAR_REGIONS {
        rows.push(AllowRow {
            case: format!("one source of {far} for a queue of {home}"),
            source_arns: vec![expected_arn_in(far, "allow-source")],
            reason: Some(ALLOW_POLICY_REGION_REASON.to_string()),
        });
        rows.push(AllowRow {
            case: format!("a source of {home} listed beside one of {far}"),
            source_arns: vec![here.clone(), expected_arn_in(far, "allow-source")],
            reason: Some(ALLOW_POLICY_REGION_REASON.to_string()),
        });
    }
    rows
}

/// A `RedriveAllowPolicy` listing a source queue outside the dead-letter queue's own region
/// is refused with the rule the LIST broke — its own sentence, not either dead-letter target
/// sentence — through both write paths, and is not stored.
#[tokio::test]
async fn a_refused_redrive_allow_policy_reports_the_source_arn_region_rule() {
    for home in HOME_REGIONS {
        for (index, row) in allow_rows(home).into_iter().enumerate() {
            let AllowRow {
                case,
                source_arns,
                reason,
            } = row;
            let router = emulator();
            let document = allow_policy(&source_arns);
            let expected =
                reason.map(|reason| refusal(&document, REDRIVE_ALLOW_POLICY, &reason));

            // 1. The SetQueueAttributes path, onto a queue carrying no permission at all.
            let name = format!("allow-dlq-{index}");
            let url = create_ok_in(&router, home, &name, &json!({})).await;
            let (status, parsed) =
                set_in(&router, home, &url, &json!({REDRIVE_ALLOW_POLICY: document})).await;
            let stored = attribute_in(&router, home, &url, REDRIVE_ALLOW_POLICY).await;
            match &expected {
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case} on SetQueueAttributes: expected the permission stored, got \
                         {status} with {parsed}"
                    );
                    assert_eq!(
                        stored.as_deref(),
                        Some(document.as_str()),
                        "case {case}: expected the queue to report the permission it took, got \
                         {stored:?}"
                    );
                }
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&parsed).as_str(), message_of(&parsed)),
                        (400, INVALID_PARAMETER_VALUE, message.clone()),
                        "case {case} on SetQueueAttributes: expected the refusal to state the \
                         rule the list broke, got {status} with {parsed}"
                    );
                    assert_eq!(
                        stored, None,
                        "case {case}: a refused permission must leave the attribute unset, got \
                         {stored:?}"
                    );
                }
            }

            // 2. The CreateQueue path, which judges the same document the same way.
            let created = format!("allow-created-{index}");
            let (status, parsed) = create_in(
                &router,
                home,
                &created,
                &json!({REDRIVE_ALLOW_POLICY: document}),
            )
            .await;
            match &expected {
                None => assert_eq!(
                    status, 200,
                    "case {case} on CreateQueue: expected the permission stored, got {status} \
                     with {parsed}"
                ),
                Some(message) => {
                    assert_eq!(
                        (status, error_code(&parsed).as_str(), message_of(&parsed)),
                        (400, INVALID_PARAMETER_VALUE, message.clone()),
                        "case {case} on CreateQueue: expected the refusal to state the rule the \
                         list broke, got {status} with {parsed}"
                    );
                    let (status, parsed) =
                        call_in(&router, home, "GetQueueUrl", &json!({"QueueName": created})).await;
                    assert_eq!(
                        (status, error_code(&parsed).as_str()),
                        (400, "QueueDoesNotExist"),
                        "case {case}: a refused create must leave no queue behind, got {status} \
                         with {parsed}"
                    );
                }
            }
        }
    }
}

/// The documents the rules ADMIT are stored and act: a same-region pair is wired, the target
/// ARN reads back intact, the dead-letter queue lists exactly that one source queue, and the
/// queue of the same name in a far region is nobody's source.
#[tokio::test]
async fn a_same_region_redrive_pair_is_wired_and_listed() {
    for home in HOME_REGIONS {
        let far = FAR_REGIONS[0];
        let router = emulator();
        let dead_letter = create_ok_in(&router, home, "wired-dlq", &json!({})).await;
        let far_dead_letter = create_ok_in(&router, far, "wired-dlq", &json!({})).await;
        let target_arn = expected_arn_in(home, "wired-dlq");
        let source_arn = expected_arn_in(home, "wired-source");

        // The permission admits the source queue of this region, and the source queue drives
        // its messages to the target of this region: both documents name only this region.
        let (status, parsed) = set_in(
            &router,
            home,
            &dead_letter,
            &json!({REDRIVE_ALLOW_POLICY: allow_policy(std::slice::from_ref(&source_arn))}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {home}: expected the same-region permission accepted, got {status} with \
             {parsed}"
        );
        let source = create_ok_in(
            &router,
            home,
            "wired-source",
            &json!({REDRIVE_POLICY: redrive_policy(&target_arn)}),
        )
        .await;

        let stored = attribute_in(&router, home, &source, REDRIVE_POLICY).await;
        assert_eq!(
            stored.as_deref(),
            Some(redrive_policy(&target_arn).as_str()),
            "case {home}: expected the wired policy reported with its target intact, got \
             {stored:?}"
        );

        let listed = ok_in(
            &router,
            home,
            "ListDeadLetterSourceQueues",
            json!({"QueueUrl": dead_letter}),
        )
        .await;
        let urls: Vec<String> = listed
            .get("queueUrls")
            .and_then(Value::as_array)
            .map(|urls| {
                urls.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            urls,
            vec![source.clone()],
            "case {home}: expected exactly the one source queue listed, got {urls:?}"
        );

        let far_listed = ok_in(
            &router,
            far,
            "ListDeadLetterSourceQueues",
            json!({"QueueUrl": far_dead_letter}),
        )
        .await;
        assert!(
            far_listed
                .get("queueUrls")
                .and_then(Value::as_array)
                .is_none_or(|urls| urls.is_empty()),
            "case {home}: the queue of the same name in {far} is nobody's dead-letter queue, got \
             {far_listed}"
        );
    }
}

/// Every redrive rule this service refuses a document for carries its OWN `Reason` sentence:
/// no two of them are reported with one sentence, and none of the dead-letter TARGET rules is
/// reported with the dead-letter queue's source-admission wording.
#[tokio::test]
async fn each_redrive_rule_has_its_own_reason_sentence() {
    // (what the rule is about, the sentence the service states it in).
    let sentences: Vec<(&str, &str)> = vec![
        ("the target's region", DEAD_LETTER_TARGET_REGION_REASON),
        ("the target's owner", DEAD_LETTER_TARGET_OWNER_REASON),
        ("the target's type", DEAD_LETTER_TARGET_TYPE_REASON),
        ("a target that is not there", DEAD_LETTER_TARGET_MISSING_REASON),
        ("the target's own permission", REDRIVE_ALLOW_POLICY_REASON),
        (
            "the permission's source ARNs",
            REDRIVE_ALLOW_POLICY_REGION_REASON,
        ),
    ];

    for (position, (rule, sentence)) in sentences.iter().enumerate() {
        for (other_rule, other) in sentences.iter().skip(position + 1) {
            assert_ne!(
                sentence, other,
                "case {rule} against {other_rule}: two rules must not share one sentence"
            );
        }
    }

    // The wording each refusal under test must carry, spelled out, and the admission sentence
    // none of the target rules may be reported with.
    assert_eq!(
        DEAD_LETTER_TARGET_REGION_REASON, TARGET_REGION_REASON,
        "case the target's region: expected Amazon SQS's own sentence, got \
         {DEAD_LETTER_TARGET_REGION_REASON:?}"
    );
    assert_eq!(
        DEAD_LETTER_TARGET_OWNER_REASON, TARGET_OWNER_REASON,
        "case the target's owner: expected Amazon SQS's own sentence, got \
         {DEAD_LETTER_TARGET_OWNER_REASON:?}"
    );
    assert_eq!(
        REDRIVE_ALLOW_POLICY_REGION_REASON, ALLOW_POLICY_REGION_REASON,
        "case the permission's source ARNs: expected Amazon SQS's own sentence, got \
         {REDRIVE_ALLOW_POLICY_REGION_REASON:?}"
    );
    for (rule, sentence) in [
        ("the target's region", DEAD_LETTER_TARGET_REGION_REASON),
        ("the target's owner", DEAD_LETTER_TARGET_OWNER_REASON),
    ] {
        assert!(
            !sentence.contains(ADMISSION_FRAGMENT),
            "case {rule}: a target rule must not be stated as the source-admission rule, got \
             {sentence:?}"
        );
    }
}
