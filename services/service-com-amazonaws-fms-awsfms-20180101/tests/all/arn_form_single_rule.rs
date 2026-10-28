// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE form rule this service applies to an Amazon
//! Resource Name, through both kinds of operation that read it.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: a string is an ARN when it carries the
//! literal `arn:` prefix, six colon-separated fields and a non-empty partition,
//! service and resource (the Region and account fields MAY be empty — ARNs of
//! global and account-less resources leave them so), and that single reading is
//! what BOTH the tagging operations' `ResourceArn` form check and the resource-set
//! membership classifier answer from: the tagging operations refuse a 400
//! `ValidationException` naming the one condition that failed for exactly the
//! strings the batch operations report as `NOT_VALID_ARN`, and for no others.
//!
//! The table in [`cases`] is the class the rule ranges over: the rows differ in
//! which condition of the form they break (no prefix, another prefix, the prefix
//! in another case, too few fields at three different counts, an empty partition,
//! service or resource), and the rows that ARE ARNs differ in whether the Region
//! and account fields are filled, in whether the resource segment itself contains
//! colons, and in partition — so the only thing the two operations are compared on
//! is the form, not one recorded value.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness lives in one place for this test target: `policy_lifecycle`
// owns it and the administrator helpers live with the gate suite — so no two
// suites drift in how a request is signed, an error envelope is read or the gate
// is opened.
use crate::administrator_gated_operations::{associate_admin, error_message};
use crate::policy_lifecycle::{call, emulator, error_code, ACCOUNT, REGION};

/// The `FailedItemReason` the classifier reports for a value that is not an ARN
/// at all (`com.amazonaws.fms#FailedItemReason`).
const NOT_VALID_ARN: &str = "NOT_VALID_ARN";

/// The resource type the test's resource set declares, so an ARN of this Region
/// and account that names a resource of this type can really be associated.
const DECLARED_TYPE: &str = "AWS::EC2::Instance";

/// One row of the ARN-form table: a name for the assertion messages, the value
/// under test, and — when the value is NOT an ARN — the part of the refusal
/// message that names the condition it breaks, or `None` when the value IS an
/// ARN.
struct Case {
    name: &'static str,
    value: String,
    defect: Option<&'static str>,
}

impl Case {
    /// Whether the shared form says this row's value is an ARN.
    fn is_arn(&self) -> bool {
        self.defect.is_none()
    }
}

/// The class the form rule is asserted over: one row per way of breaking each
/// condition, and one row per shape of ARN that satisfies all of them.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no-prefix-at-all",
            value: "this-is-not-an-arn-at-all".to_string(),
            defect: Some("must start with 'arn:'"),
        },
        Case {
            name: "another-prefix",
            value: format!("aws:arn:fms:{REGION}:{ACCOUNT}:policy/abc"),
            defect: Some("must start with 'arn:'"),
        },
        Case {
            // The prefix is the LITERAL `arn:`, so another case is another
            // prefix.
            name: "prefix-in-another-case",
            value: format!("ARN:aws:fms:{REGION}:{ACCOUNT}:policy/abc"),
            defect: Some("must start with 'arn:'"),
        },
        Case {
            name: "the-prefix-and-nothing-else",
            value: "arn:".to_string(),
            defect: Some("six colon-separated fields, found 2"),
        },
        Case {
            name: "three-fields",
            value: "arn:aws:fms".to_string(),
            defect: Some("six colon-separated fields, found 3"),
        },
        Case {
            name: "five-fields",
            value: format!("arn:aws:ec2:{REGION}:{ACCOUNT}"),
            defect: Some("six colon-separated fields, found 5"),
        },
        Case {
            name: "empty-partition",
            value: format!("arn::fms:{REGION}:{ACCOUNT}:policy/abc"),
            defect: Some("must name a partition"),
        },
        Case {
            // Six fields, every one of them empty: the first condition that
            // fails is the partition.
            name: "six-empty-fields",
            value: "arn:::::".to_string(),
            defect: Some("must name a partition"),
        },
        Case {
            name: "empty-service",
            value: format!("arn:aws::{REGION}:{ACCOUNT}:policy/abc"),
            defect: Some("must name a service"),
        },
        Case {
            name: "empty-resource",
            value: format!("arn:aws:fms:{REGION}:{ACCOUNT}:"),
            defect: Some("must name a resource"),
        },
        Case {
            name: "every-field-filled",
            value: format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-00000000000000001"),
            defect: None,
        },
        Case {
            // A global service's ARN leaves the Region and the account empty.
            name: "empty-region-and-account",
            value: "arn:aws:s3:::example-bucket".to_string(),
            defect: None,
        },
        Case {
            // An account-scoped global service's ARN leaves only the Region
            // empty.
            name: "empty-region-only",
            value: format!("arn:aws:iam::{ACCOUNT}:role/example"),
            defect: None,
        },
        Case {
            // The resource segment is everything after the fifth colon, so its
            // own colons do not add fields.
            name: "colons-inside-the-resource",
            value: format!(
                "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT}:loadbalancer/app/edge:1"
            ),
            defect: None,
        },
        Case {
            name: "another-partition",
            value: format!("arn:aws-cn:ec2:cn-north-1:{ACCOUNT}:instance/i-00000000000000002"),
            defect: None,
        },
    ]
}

/// The tagging operations that run the `ResourceArn` form check, each with the
/// rest of the request body it needs.
fn tagging_calls(value: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "ResourceArn": value, "TagList": [{ "Key": "stage", "Value": "test" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": value, "TagKeys": ["stage"] }),
        ),
        (
            "ListTagsForResource",
            json!({ "ResourceArn": value }),
        ),
    ]
}

/// Create a resource set declaring [`DECLARED_TYPE`] and hand back its `Id`.
async fn create_set(router: &MultiServiceRouter) -> String {
    let (status, created) = call(
        router,
        "PutResourceSet",
        &json!({
            "ResourceSet": {
                "Name": "arn-form-single-rule",
                "ResourceTypeList": [DECLARED_TYPE],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected PutResourceSet to answer 200, got {status} with {created}"
    );
    let id = created["ResourceSet"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "expected PutResourceSet to mint an Id, got {created}"
    );
    id
}

/// The `URI -> Reason` pairs a `FailedItems` list reports.
fn failed_reasons(body: &Value) -> BTreeMap<String, String> {
    body["FailedItems"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["URI"].as_str().unwrap_or_default().to_string(),
                        item["Reason"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Associate every row's value with a resource set in one batch and report what
/// `BatchAssociateResource` said about each.
async fn associate_every_row(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let id = create_set(router).await;
    let items: Vec<String> = cases().into_iter().map(|case| case.value).collect();
    let (status, body) = call(
        router,
        "BatchAssociateResource",
        &json!({ "ResourceSetIdentifier": id, "Items": items }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected BatchAssociateResource to answer 200 for the whole table, got {status} with \
         {body}"
    );
    failed_reasons(&body)
}

/// Every operation that checks the FORM of a `ResourceArn` refuses exactly the
/// values that are not ARNs, with 400 `ValidationException` naming the one
/// condition that failed — and answers a value that IS an ARN from the store
/// instead, which for an ARN naming no Firewall Manager resource is the modelled
/// `ResourceNotFoundException`.
#[tokio::test]
async fn tagging_operations_refuse_exactly_the_values_that_are_not_arns() {
    let router = emulator();
    for case in cases() {
        for (operation, body) in tagging_calls(&case.value) {
            let label = format!("{} / {operation}", case.name);
            let (status, answered) = call(&router, operation, &body).await;
            match case.defect {
                Some(defect) => {
                    assert_eq!(
                        (status, error_code(&answered).as_str()),
                        (400, "ValidationException"),
                        "case {label:?}: expected {operation}({:?}) to answer 400 \
                         ValidationException, got {status} with {answered:?}",
                        case.value
                    );
                    let message = error_message(&answered);
                    assert!(
                        message.contains(defect) && message.contains(&case.value),
                        "case {label:?}: expected the refusal to name {defect:?} and the value, \
                         got {message:?}"
                    );
                }
                None => {
                    assert_eq!(
                        (status, error_code(&answered).as_str()),
                        (400, "ResourceNotFoundException"),
                        "case {label:?}: {:?} is an ARN, so {operation} must look it up rather \
                         than refuse its form, got {status} with {answered:?}",
                        case.value
                    );
                }
            }
        }
    }
}

/// The resource-set membership classifier reads the SAME form: it reports
/// `NOT_VALID_ARN` for exactly the values that are not ARNs, and judges a value
/// that IS an ARN on its later fields — never on its form.
#[tokio::test]
async fn resource_set_association_reports_not_valid_arn_for_exactly_the_same_values() {
    let router = emulator();
    associate_admin(&router).await;
    let reported = associate_every_row(&router).await;

    for case in cases() {
        let reason = reported.get(&case.value).map(String::as_str);
        if case.is_arn() {
            assert_ne!(
                reason,
                Some(NOT_VALID_ARN),
                "case {:?}: {:?} is an ARN, so the classifier must weigh its fields rather than \
                 its form, got {reason:?}",
                case.name,
                case.value
            );
        } else {
            assert_eq!(
                reason,
                Some(NOT_VALID_ARN),
                "case {:?}: expected the classifier to report {NOT_VALID_ARN} for {:?}, got \
                 {reason:?}",
                case.name,
                case.value
            );
        }
    }

    // An ARN of the Region, account and declared type the set accepts is
    // associated outright, so "not reported as a failure" is really a membership
    // and not a silently dropped item.
    let member = format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-00000000000000001");
    assert_eq!(
        reported.get(&member),
        None,
        "case \"every-field-filled\": expected {member:?} to associate, got {:?}",
        reported.get(&member)
    );
}

/// The two readings cannot drift: the set of values the tagging operations refuse
/// on form is exactly the set the batch classifier calls `NOT_VALID_ARN`,
/// whatever either of them says about any single value.
#[tokio::test]
async fn the_form_check_and_the_classifier_agree_on_every_value() {
    let router = emulator();
    associate_admin(&router).await;
    let reported = associate_every_row(&router).await;

    let mut refused_on_form: Vec<String> = Vec::new();
    let mut classified_not_an_arn: Vec<String> = Vec::new();
    for case in cases() {
        let (_, answered) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceArn": &case.value }),
        )
        .await;
        if error_code(&answered) == "ValidationException" {
            refused_on_form.push(case.value.clone());
        }
        if reported.get(&case.value).map(String::as_str) == Some(NOT_VALID_ARN) {
            classified_not_an_arn.push(case.value.clone());
        }
    }

    assert_eq!(
        refused_on_form, classified_not_an_arn,
        "the tagging form check and the membership classifier must call the same values ARNs: the \
         form check refused {refused_on_form:?}, the classifier refused {classified_not_an_arn:?}"
    );
    assert!(
        !refused_on_form.is_empty() && refused_on_form.len() < cases().len(),
        "the table must span both answers, but {} of {} rows were refused on form",
        refused_on_form.len(),
        cases().len()
    );
}
