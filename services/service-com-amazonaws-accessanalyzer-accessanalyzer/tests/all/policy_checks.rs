// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CheckNoNewAccess`, driven over the wire.
//!
//! Every case goes through the real router: an `http::Request` is dispatched to the
//! mounted service, so restJson1 routing, boundary constraint validation, the handler
//! and the response serialization all run exactly as they do in the emulator binary.
//! The harness is the shared one in `tests/all/harness.rs`, so there is a single
//! emulator setup in this crate.
//!
//! The rule under test: the check answers `FAIL` exactly when some access request — an
//! action on a resource, for a principal, in a request context — is allowed by the
//! updated policy and not allowed by the existing policy under IAM's own evaluation
//! rules (an explicit `Deny` beats every `Allow`; anything no `Allow` matches is
//! denied), and `PASS` otherwise. The tables below span what that rule generalises over:
//! which element widens (action, resource, principal, effect, condition), the form the
//! element takes (single string, list, wildcard, negated `Not…` element, single
//! statement object vs array), the condition operator family (string, wildcard,
//! numeric, date, boolean, presence, `…IfExists`) and the direction of the change.

use serde_json::{json, Value};

use crate::harness::{Emulator, Reply};

/// The route the operation is bound to, from `OPERATIONS.md`:
/// `POST /policy/check-no-new-access -> 200`.
const ROUTE: &str = "/policy/check-no-new-access";

/// One comparison, and what the rule says it must answer.
struct Case<'a> {
    /// What the row varies, for the assertion messages.
    name: &'a str,
    /// The modelled `AccessCheckPolicyType` the documents are read as.
    policy_type: &'static str,
    existing: Value,
    updated: Value,
    /// The `CheckNoNewAccessResult` the rule demands.
    expect: &'static str,
    /// For a `FAIL`, the zero-based indexes of the updated policy's statements that
    /// allow the new access; empty for a `PASS`.
    expect_statements: &'static [i64],
}

/// Run one check over the wire.
async fn check(emulator: &Emulator, case: &Case<'_>) -> Reply {
    emulator
        .send(
            "POST",
            ROUTE,
            Some(json!({
                // Both documents travel as JSON *strings*, as
                // `AccessCheckPolicyDocument` is modelled.
                "existingPolicyDocument": case.existing.to_string(),
                "newPolicyDocument": case.updated.to_string(),
                "policyType": case.policy_type,
            })),
        )
        .await
}

/// The `statementIndex` of every reason the reply carries, in reply order.
fn reported_statements(reply: &Reply) -> Vec<i64> {
    reply
        .member("reasons")
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .map(|reason| {
                    reason
                        .get("statementIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(i64::MIN)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one row of a table.
async fn assert_case(emulator: &Emulator, case: &Case<'_>) {
    let reply = check(emulator, case).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected HTTP 200, got {} with code {:?} and body {:?}",
        case.name,
        reply.status,
        reply.code(),
        reply.body
    );
    let result = reply
        .member("result")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        result, case.expect,
        "case {:?}: expected result {:?}, got {:?} with body {:?}",
        case.name, case.expect, result, reply.body
    );
    let message = reply
        .member("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty(),
        "case {:?}: expected a message explaining the result, got {:?}",
        case.name,
        reply.body
    );
    let reported = reported_statements(&reply);
    assert_eq!(
        reported,
        case.expect_statements.to_vec(),
        "case {:?}: expected the reasons to name statements {:?}, got {:?} from body {:?}",
        case.name,
        case.expect_statements,
        reported,
        reply.body
    );
    if case.expect == "FAIL" {
        for reason in reply
            .member("reasons")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            let description = reason.get("description").and_then(Value::as_str);
            assert!(
                description.is_some_and(|text| !text.is_empty()),
                "case {:?}: every reason carries a description, got {:?}",
                case.name,
                reason
            );
        }
    }
}

/// An identity-policy statement allowing `actions` on `resources`.
fn allow(actions: Value, resources: Value) -> Value {
    json!({"Effect": "Allow", "Action": actions, "Resource": resources})
}

/// A whole identity policy wrapping `statements`.
fn identity_policy(statements: Value) -> Value {
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// A single-statement identity policy allowing `actions` on `resources`.
fn identity_allow(actions: Value, resources: Value) -> Value {
    identity_policy(json!([allow(actions, resources)]))
}

/// A single-statement identity policy allowing `s3:GetObject` on one bucket, under
/// `condition` (the `Condition` block, or `null` for no condition).
fn conditional_allow(condition: Value) -> Value {
    let mut statement = allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*"));
    if !condition.is_null() {
        statement
            .as_object_mut()
            .expect("a statement object")
            .insert("Condition".to_string(), condition);
    }
    identity_policy(json!([statement]))
}

#[tokio::test]
async fn identity_policy_comparisons_follow_the_access_rule() {
    let emulator = Emulator::new();
    let bucket = "arn:aws:s3:::reports/*";

    let cases = vec![
        Case {
            name: "the same statement, byte for byte",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_allow(json!("s3:GetObject"), json!(bucket)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "one statement object instead of a one-element array",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_policy(allow(json!(["s3:GetObject"]), json!([bucket]))),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "an action dropped from the list",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!(["s3:GetObject", "s3:PutObject"]), json!(bucket)),
            updated: identity_allow(json!(["s3:GetObject"]), json!(bucket)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "an action added to the list",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!(["s3:GetObject"]), json!(bucket)),
            updated: identity_allow(json!(["s3:GetObject", "s3:PutObject"]), json!(bucket)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "the action wildcard widened",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:Get*"), json!(bucket)),
            updated: identity_allow(json!("s3:*"), json!(bucket)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "the action wildcard narrowed",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:*"), json!(bucket)),
            updated: identity_allow(json!("s3:Get*"), json!(bucket)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "the action spelled in another case",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_allow(json!("S3:GETOBJECT"), json!(bucket)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "the resource widened to every resource",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_allow(json!("s3:GetObject"), json!("*")),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "the resource narrowed to a prefix",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/2024/*")),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            // ARNs are case-sensitive, unlike action names.
            name: "the resource ARN spelled in another case",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            updated: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::Reports/*")),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "NotAction narrowed into a plain Action",
            policy_type: "IDENTITY_POLICY",
            existing: identity_policy(json!([{
                "Effect": "Allow",
                "NotAction": ["s3:DeleteObject"],
                "Resource": bucket,
            }])),
            updated: identity_allow(json!("s3:GetObject"), json!(bucket)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "NotAction widened to every action",
            policy_type: "IDENTITY_POLICY",
            existing: identity_policy(json!([{
                "Effect": "Allow",
                "NotAction": ["s3:DeleteObject"],
                "Resource": bucket,
            }])),
            updated: identity_allow(json!("*"), json!(bucket)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "an explicit Deny added over the same Allow",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:*"), json!(bucket)),
            updated: identity_policy(json!([
                allow(json!("s3:*"), json!(bucket)),
                {"Effect": "Deny", "Action": "s3:DeleteObject", "Resource": bucket},
            ])),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "an explicit Deny dropped from the policy",
            policy_type: "IDENTITY_POLICY",
            existing: identity_policy(json!([
                allow(json!("s3:*"), json!(bucket)),
                {"Effect": "Deny", "Action": "s3:DeleteObject", "Resource": bucket},
            ])),
            updated: identity_allow(json!("s3:*"), json!(bucket)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "a second statement grants the new access",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!(bucket)),
            updated: identity_policy(json!([
                allow(json!("s3:GetObject"), json!(bucket)),
                allow(json!("s3:PutObject"), json!(bucket)),
            ])),
            expect: "FAIL",
            expect_statements: &[1],
        },
        Case {
            name: "both statements grant new access over an unrelated policy",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(
                json!("logs:DescribeLogGroups"),
                json!("arn:aws:logs:us-east-1:111122223333:log-group:*"),
            ),
            updated: identity_policy(json!([
                allow(json!("s3:GetObject"), json!(bucket)),
                allow(json!("sqs:SendMessage"), json!("arn:aws:sqs:us-east-1:111122223333:jobs")),
            ])),
            expect: "FAIL",
            expect_statements: &[0, 1],
        },
        Case {
            name: "a statement with a Sid is reported by index",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(
                json!("logs:DescribeLogGroups"),
                json!("arn:aws:logs:us-east-1:111122223333:log-group:*"),
            ),
            updated: identity_policy(json!([{
                "Sid": "AllowReads",
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": bucket,
            }])),
            expect: "FAIL",
            expect_statements: &[0],
        },
    ];

    for case in &cases {
        assert_case(&emulator, case).await;
    }
}

#[tokio::test]
async fn condition_changes_follow_the_access_rule() {
    let emulator = Emulator::new();

    // One row per condition operator family, in both directions: adding a constraint
    // narrows the policy (PASS), relaxing one widens it (FAIL).
    let narrower_then_wider: Vec<(&'static str, Value, Value)> = vec![
        (
            "a StringEquals condition",
            conditional_allow(json!({"StringEquals": {"aws:PrincipalOrgID": "o-exampleorgid"}})),
            conditional_allow(Value::Null),
        ),
        (
            "a StringEquals value list",
            conditional_allow(json!({"StringEquals": {"aws:PrincipalOrgID": ["o-exampleorgid"]}})),
            conditional_allow(
                json!({"StringEquals": {"aws:PrincipalOrgID": ["o-exampleorgid", "o-secondorgid"]}}),
            ),
        ),
        (
            "a StringLike principal pattern",
            conditional_allow(json!({"StringLike": {
                "aws:PrincipalArn": "arn:aws:iam::111122223333:role/app-*"
            }})),
            conditional_allow(json!({"StringLike": {
                "aws:PrincipalArn": "arn:aws:iam::111122223333:role/*"
            }})),
        ),
        (
            "an ArnLike source pattern",
            conditional_allow(json!({"ArnLike": {
                "aws:SourceArn": "arn:aws:cloudtrail:us-east-1:111122223333:trail/audit-*"
            }})),
            conditional_allow(json!({"ArnLike": {
                "aws:SourceArn": "arn:aws:cloudtrail:us-east-1:111122223333:trail/*"
            }})),
        ),
        (
            "a numeric point value against a bound",
            conditional_allow(json!({"NumericEquals": {"s3:max-keys": 5}})),
            conditional_allow(json!({"NumericLessThan": {"s3:max-keys": 10}})),
        ),
        (
            "a numeric bound that includes its endpoint",
            conditional_allow(json!({"NumericLessThan": {"s3:max-keys": 10}})),
            conditional_allow(json!({"NumericLessThanEquals": {"s3:max-keys": 10}})),
        ),
        (
            // A numeric condition value is a number of any magnitude (a byte count, an
            // object count), not a timestamp, so a large bound is compared, not dropped.
            "a numeric bound far beyond the timestamp range",
            conditional_allow(json!({"NumericLessThan": {"s3:max-keys": 1_000_000_000_000_000u64}})),
            conditional_allow(json!({"NumericLessThan": {"s3:max-keys": 2_000_000_000_000_000u64}})),
        ),
        (
            "a date bound moved later",
            conditional_allow(json!({"DateLessThan": {"aws:CurrentTime": "2024-01-01T00:00:00Z"}})),
            conditional_allow(json!({"DateLessThan": {"aws:CurrentTime": "2025-01-01T00:00:00Z"}})),
        ),
        (
            "a Bool condition dropped",
            conditional_allow(json!({"Bool": {"aws:SecureTransport": "true"}})),
            conditional_allow(Value::Null),
        ),
        (
            "a Null presence check dropped",
            conditional_allow(json!({"Null": {"aws:MultiFactorAuthPresent": "false"}})),
            conditional_allow(Value::Null),
        ),
        (
            "a StringEquals relaxed with IfExists",
            conditional_allow(json!({"StringEquals": {"aws:PrincipalOrgID": "o-exampleorgid"}})),
            conditional_allow(
                json!({"StringEqualsIfExists": {"aws:PrincipalOrgID": "o-exampleorgid"}}),
            ),
        ),
        (
            "a StringNotEquals exclusion dropped",
            conditional_allow(
                json!({"StringNotEquals": {"aws:PrincipalAccount": "444455556666"}}),
            ),
            conditional_allow(Value::Null),
        ),
    ];

    for (name, narrower, wider) in &narrower_then_wider {
        assert_case(
            &emulator,
            &Case {
                name,
                policy_type: "IDENTITY_POLICY",
                existing: narrower.clone(),
                updated: wider.clone(),
                expect: "FAIL",
                expect_statements: &[0],
            },
        )
        .await;
        assert_case(
            &emulator,
            &Case {
                name,
                policy_type: "IDENTITY_POLICY",
                existing: wider.clone(),
                updated: narrower.clone(),
                expect: "PASS",
                expect_statements: &[],
            },
        )
        .await;
        assert_case(
            &emulator,
            &Case {
                name,
                policy_type: "IDENTITY_POLICY",
                existing: narrower.clone(),
                updated: narrower.clone(),
                expect: "PASS",
                expect_statements: &[],
            },
        )
        .await;
    }
}

#[tokio::test]
async fn resource_policy_principals_follow_the_access_rule() {
    let emulator = Emulator::new();
    let queue = "arn:aws:sqs:us-east-1:111122223333:jobs";
    let role = "arn:aws:iam::111122223333:role/app";
    let other_role = "arn:aws:iam::111122223333:role/batch";

    /// A resource-policy statement granting `principal` `sqs:SendMessage` on the queue.
    fn resource_policy(principal: Value, resource: Option<&str>) -> Value {
        let mut statement = json!({
            "Effect": "Allow",
            "Principal": principal,
            "Action": "sqs:SendMessage",
        });
        if let Some(resource) = resource {
            statement
                .as_object_mut()
                .expect("a statement object")
                .insert("Resource".to_string(), json!(resource));
        }
        json!({"Version": "2012-10-17", "Statement": [statement]})
    }

    let cases = vec![
        Case {
            name: "the same principal",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": role}), Some(queue)),
            updated: resource_policy(json!({"AWS": role}), Some(queue)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "a second principal added",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": role}), Some(queue)),
            updated: resource_policy(json!({"AWS": [role, other_role]}), Some(queue)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "a principal removed",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": [role, other_role]}), Some(queue)),
            updated: resource_policy(json!({"AWS": [role]}), Some(queue)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "the principal opened to everyone",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": role}), Some(queue)),
            updated: resource_policy(json!("*"), Some(queue)),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            name: "everyone narrowed to one principal",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": "*"}), Some(queue)),
            updated: resource_policy(json!({"AWS": role}), Some(queue)),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            // An account id and that account's root ARN name the same principal, per
            // the IAM grammar's Principal element.
            name: "an account id respelled as its root ARN",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": "111122223333"}), Some(queue)),
            updated: resource_policy(
                json!({"AWS": "arn:aws:iam::111122223333:root"}),
                Some(queue),
            ),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "a service principal added",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"Service": "s3.amazonaws.com"}), Some(queue)),
            updated: resource_policy(
                json!({"Service": ["s3.amazonaws.com", "lambda.amazonaws.com"]}),
                Some(queue),
            ),
            expect: "FAIL",
            expect_statements: &[0],
        },
        Case {
            // A resource policy need not name its own resource; the comparison is then
            // over actions, principals and conditions alone.
            name: "no Resource element on either side",
            policy_type: "RESOURCE_POLICY",
            existing: resource_policy(json!({"AWS": role}), None),
            updated: resource_policy(json!({"AWS": role}), None),
            expect: "PASS",
            expect_statements: &[],
        },
        Case {
            name: "a NotPrincipal exclusion dropped",
            policy_type: "RESOURCE_POLICY",
            existing: json!({"Statement": [{
                "Effect": "Allow",
                "NotPrincipal": {"AWS": other_role},
                "Action": "sqs:SendMessage",
                "Resource": queue,
            }]}),
            updated: resource_policy(json!("*"), Some(queue)),
            expect: "FAIL",
            expect_statements: &[0],
        },
    ];

    for case in &cases {
        assert_case(&emulator, case).await;
    }
}

/// The comparison over a bounded lattice of policies: every pair of a 3x3 grid.
///
/// The grid is the product of two chains, each strictly widening — the action chain
/// `s3:GetObject` < `s3:Get*` < `s3:*` and the resource chain
/// `reports/2024/*` < `reports/*` < `*` — so one policy's access is contained in
/// another's exactly when it is below it in BOTH coordinates. That containment is the
/// rule, so the expected answer for all 81 ordered pairs is computed from the
/// coordinates rather than listed: `PASS` iff the updated policy's coordinates are
/// componentwise less than or equal to the existing policy's. Bounded and exhaustive
/// over the grid, so there is no seed to print.
#[tokio::test]
async fn containment_over_a_policy_lattice_decides_the_result() {
    let emulator = Emulator::new();
    let actions = ["s3:GetObject", "s3:Get*", "s3:*"];
    let resources = [
        "arn:aws:s3:::reports/2024/*",
        "arn:aws:s3:::reports/*",
        "arn:aws:s3:::*",
    ];

    for (existing_action, existing_resource) in (0..actions.len())
        .flat_map(|action| (0..resources.len()).map(move |resource| (action, resource)))
    {
        for (updated_action, updated_resource) in (0..actions.len())
            .flat_map(|action| (0..resources.len()).map(move |resource| (action, resource)))
        {
            let contained =
                updated_action <= existing_action && updated_resource <= existing_resource;
            let name: &'static str = if contained {
                "a policy at or below the existing one in the lattice"
            } else {
                "a policy above the existing one in some coordinate"
            };
            let case = Case {
                name,
                policy_type: "IDENTITY_POLICY",
                existing: identity_allow(
                    json!(actions[existing_action]),
                    json!(resources[existing_resource]),
                ),
                updated: identity_allow(
                    json!(actions[updated_action]),
                    json!(resources[updated_resource]),
                ),
                expect: if contained { "PASS" } else { "FAIL" },
                expect_statements: if contained { &[] } else { &[0] },
            };
            let reply = check(&emulator, &case).await;
            let result = reply
                .member("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                result, case.expect,
                "case {:?}: existing ({}, {}) vs updated ({}, {}): expected {:?}, got {:?} with body {:?}",
                case.name,
                actions[existing_action],
                resources[existing_resource],
                actions[updated_action],
                resources[updated_resource],
                case.expect,
                result,
                reply.body
            );
            assert_eq!(
                reported_statements(&reply),
                case.expect_statements.to_vec(),
                "case {:?}: existing ({}, {}) vs updated ({}, {}): unexpected reasons {:?}",
                case.name,
                actions[existing_action],
                resources[existing_resource],
                actions[updated_action],
                resources[updated_resource],
                reply.body
            );
        }
    }
}

/// A document the engine cannot read, or cannot decide, is refused — never answered
/// with a guessed `PASS` or `FAIL`.
#[tokio::test]
async fn documents_that_cannot_be_compared_are_refused() {
    let emulator = Emulator::new();
    let good = identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")).to_string();

    // name, existing document, updated document, expected status, expected error code.
    let rows: Vec<(&str, String, String, u16, &str)> = vec![
        (
            "the existing document is not JSON",
            "not a policy at all".to_string(),
            good.clone(),
            400,
            "ValidationException",
        ),
        (
            "the updated document is a JSON array",
            good.clone(),
            "[]".to_string(),
            400,
            "ValidationException",
        ),
        (
            "no Statement element",
            good.clone(),
            json!({"Version": "2012-10-17"}).to_string(),
            400,
            "ValidationException",
        ),
        (
            "an empty Statement array",
            good.clone(),
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
            400,
            "ValidationException",
        ),
        (
            "a statement with no Effect",
            good.clone(),
            json!({"Statement": [{"Action": "s3:GetObject", "Resource": "*"}]}).to_string(),
            400,
            "ValidationException",
        ),
        (
            "a statement with no Action",
            good.clone(),
            json!({"Statement": [{"Effect": "Allow", "Resource": "*"}]}).to_string(),
            400,
            "ValidationException",
        ),
        (
            "a statement carrying both Action and NotAction",
            good.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "NotAction": "s3:PutObject",
                "Resource": "*",
            }]})
            .to_string(),
            400,
            "ValidationException",
        ),
        (
            "an identity policy statement carrying a Principal",
            good.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::111122223333:role/app"},
                "Action": "s3:GetObject",
                "Resource": "*",
            }]})
            .to_string(),
            400,
            "ValidationException",
        ),
        (
            // Modelled but not decidable here: the emulator refuses rather than
            // guessing a result for a condition operator it does not evaluate.
            "a condition operator the engine does not evaluate",
            good.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*",
                "Condition": {"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}},
            }]})
            .to_string(),
            422,
            "UnprocessableEntityException",
        ),
        (
            "a multivalued set condition operator",
            good.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*",
                "Condition": {"ForAnyValue:StringEquals": {"aws:TagKeys": "team"}},
            }]})
            .to_string(),
            422,
            "UnprocessableEntityException",
        ),
    ];

    for (name, existing, updated, status, code) in rows {
        let reply = emulator
            .send(
                "POST",
                ROUTE,
                Some(json!({
                    "existingPolicyDocument": existing,
                    "newPolicyDocument": updated,
                    "policyType": "IDENTITY_POLICY",
                })),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {name:?}: expected HTTP {status}, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            code,
            "case {name:?}: expected error code {code:?}, got {:?} with body {:?}",
            reply.code(),
            reply.body
        );
        assert!(
            reply.member("result").is_none(),
            "case {name:?}: a refused comparison carries no result, got {:?}",
            reply.body
        );
    }
}

/// A refused document says WHICH member was wrong and why, in the modelled
/// `ValidationException`'s own members.
#[tokio::test]
async fn a_refused_document_names_the_member_that_was_wrong() {
    let emulator = Emulator::new();
    let good = identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")).to_string();

    // name, the member expected in `fieldList`, the request body.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the existing document",
            "existingPolicyDocument",
            json!({
                "existingPolicyDocument": "{",
                "newPolicyDocument": good.clone(),
                "policyType": "IDENTITY_POLICY",
            }),
        ),
        (
            "the updated document",
            "newPolicyDocument",
            json!({
                "existingPolicyDocument": good.clone(),
                "newPolicyDocument": "{",
                "policyType": "IDENTITY_POLICY",
            }),
        ),
    ];

    for (name, member, body) in rows {
        let reply = emulator.send("POST", ROUTE, Some(body)).await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some("cannotParse"),
            "case {name:?}: a document that is not JSON cannot be parsed, got {:?}",
            reply.body
        );
        let named: Vec<String> = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| {
                        field
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![member.to_string()],
            "case {name:?}: expected fieldList to name {member:?}, got {named:?} from {:?}",
            reply.body
        );
    }
}

/// Each policy grammar requires the elements it needs to mean anything, so a statement
/// missing one is refused rather than read as unconstrained.
#[tokio::test]
async fn each_flavor_requires_the_elements_its_grammar_needs() {
    let emulator = Emulator::new();
    let good_identity =
        identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")).to_string();
    let good_resource = json!({"Statement": [{
        "Effect": "Allow",
        "Principal": {"AWS": "arn:aws:iam::111122223333:role/app"},
        "Action": "sqs:SendMessage",
    }]})
    .to_string();

    // name, policyType, the good document, the document under test.
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "an identity policy statement with no Resource",
            "IDENTITY_POLICY",
            good_identity.clone(),
            json!({"Statement": [{"Effect": "Allow", "Action": "s3:GetObject"}]}).to_string(),
        ),
        (
            "a resource policy statement with no Principal",
            "RESOURCE_POLICY",
            good_resource.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": "sqs:SendMessage",
                "Resource": "arn:aws:sqs:us-east-1:111122223333:jobs",
            }]})
            .to_string(),
        ),
        (
            "a statement carrying both Principal and NotPrincipal",
            "RESOURCE_POLICY",
            good_resource.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::111122223333:role/app"},
                "NotPrincipal": {"AWS": "arn:aws:iam::111122223333:role/batch"},
                "Action": "sqs:SendMessage",
            }]})
            .to_string(),
        ),
        (
            "a Principal map keying something that is not a principal type",
            "RESOURCE_POLICY",
            good_resource.clone(),
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"Robot": "arn:aws:iam::111122223333:role/app"},
                "Action": "sqs:SendMessage",
            }]})
            .to_string(),
        ),
    ];

    for (name, policy_type, good, under_test) in rows {
        let reply = emulator
            .send(
                "POST",
                ROUTE,
                Some(json!({
                    "existingPolicyDocument": good,
                    "newPolicyDocument": under_test,
                    "policyType": policy_type,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {name:?}: expected a ValidationException, got {:?} with body {:?}",
            reply.code(),
            reply.body
        );
    }
}

/// Everything about the comparison's size is derived from the two documents, so each
/// ceiling is REFUSED at, not grown into. The caps come from the engine itself, so the
/// documents here are built one past whatever they are.
#[tokio::test]
async fn a_comparison_past_the_engine_ceilings_is_refused() {
    use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::check_no_new_access::policy;

    let emulator = Emulator::new();
    let good = identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")).to_string();

    // One statement per cap, each built one past it.
    let too_long = {
        let filler = "a".repeat(policy::MAX_DOCUMENT_BYTES);
        identity_allow(
            json!("s3:GetObject"),
            json!(format!("arn:aws:s3:::reports/{filler}")),
        )
        .to_string()
    };
    let too_many_statements = identity_policy(Value::Array(
        (0..=policy::MAX_STATEMENTS)
            .map(|index| allow(json!("s3:GetObject"), json!(format!("arn:aws:s3:::b{index}/*"))))
            .collect(),
    ))
    .to_string();
    let too_many_action_values = identity_allow(
        Value::Array(
            (0..=policy::MAX_ELEMENT_VALUES)
                .map(|index| json!(format!("s3:Get{index}")))
                .collect(),
        ),
        json!("arn:aws:s3:::reports/*"),
    )
    .to_string();
    let too_many_condition_values = {
        let values: Vec<Value> = (0..=policy::MAX_CONDITION_VALUES)
            .map(|index| json!(format!("o-org{index}")))
            .collect();
        conditional_allow(json!({"StringEquals": {"aws:PrincipalOrgID": values}})).to_string()
    };
    let condition_value_too_long = conditional_allow(json!({"StringEquals": {
        "aws:PrincipalOrgID": "o-".to_string() + &"g".repeat(policy::MAX_CONDITION_VALUE_BYTES)
    }}))
    .to_string();
    // Six keys with ten values each: the request contexts are the product over the keys,
    // which is far past MAX_CONTEXTS, so the comparison is refused instead of enumerated.
    let too_many_contexts = {
        let mut block = serde_json::Map::new();
        let mut equals = serde_json::Map::new();
        for key in 0..6 {
            let values: Vec<Value> = (0..10).map(|value| json!(format!("v{key}-{value}"))).collect();
            equals.insert(format!("aws:PrincipalTag/team{key}"), Value::Array(values));
        }
        block.insert("StringEquals".to_string(), Value::Object(equals));
        conditional_allow(Value::Object(block)).to_string()
    };

    // name, the document under test, expected status, expected error code.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a document past the document-length ceiling",
            too_long,
            400,
            "ValidationException",
        ),
        (
            "a document past the statement ceiling",
            too_many_statements,
            400,
            "ValidationException",
        ),
        (
            "an element past the element-value ceiling",
            too_many_action_values,
            400,
            "ValidationException",
        ),
        (
            "a condition key past the condition-value ceiling",
            too_many_condition_values,
            400,
            "ValidationException",
        ),
        (
            "a condition value past the value-length ceiling",
            condition_value_too_long,
            400,
            "ValidationException",
        ),
        (
            "a condition product past the request-context ceiling",
            too_many_contexts,
            422,
            "UnprocessableEntityException",
        ),
    ];

    for (name, under_test, status, code) in rows {
        let reply = emulator
            .send(
                "POST",
                ROUTE,
                Some(json!({
                    "existingPolicyDocument": good.clone(),
                    "newPolicyDocument": under_test,
                    "policyType": "IDENTITY_POLICY",
                })),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {name:?}: expected HTTP {status}, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            code,
            "case {name:?}: expected error code {code:?}, got {:?} with body {:?}",
            reply.code(),
            reply.body
        );
        assert!(
            reply.member("result").is_none(),
            "case {name:?}: a refused comparison carries no result, got {:?}",
            reply.body
        );
    }
}

/// A condition value that is not a number or a timestamp — including the spellings
/// `f64` accepts but no policy can mean — is answered, never crashed on: such a value
/// satisfies no comparison, so the statement carrying it allows nothing.
#[tokio::test]
async fn condition_values_that_are_not_numbers_or_times_are_answered() {
    let emulator = Emulator::new();

    // The operator family, the value, and what the comparison must answer when the
    // UPDATED policy carries that constraint and the existing one is unconditional.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("DateLessThan", "inf", json!("inf")),
        ("DateLessThan", "-inf", json!("-inf")),
        ("DateLessThan", "NaN", json!("NaN")),
        ("DateLessThan", "1e300", json!("1e300")),
        ("DateLessThan", "not a date", json!("not a date")),
        ("DateGreaterThan", "inf", json!("inf")),
        ("NumericLessThan", "inf", json!("inf")),
        ("NumericEquals", "NaN", json!("NaN")),
        ("NumericLessThan", "1e400", json!("1e400")),
    ];

    for (operator, label, value) in rows {
        let name = format!("{operator} {label}");
        // A constraint nothing satisfies allows nothing, so the updated policy grants no
        // new access over an unconditional existing one.
        assert_case(
            &emulator,
            &Case {
                name: &name,
                policy_type: "IDENTITY_POLICY",
                existing: conditional_allow(Value::Null),
                updated: conditional_allow(json!({operator: {"aws:CurrentTime": value}})),
                expect: "PASS",
                expect_statements: &[],
            },
        )
        .await;
        // And the unconditional policy allows access the constrained one does not.
        assert_case(
            &emulator,
            &Case {
                name: &name,
                policy_type: "IDENTITY_POLICY",
                existing: conditional_allow(json!({operator: {"aws:CurrentTime": value}})),
                updated: conditional_allow(Value::Null),
                expect: "FAIL",
                expect_statements: &[0],
            },
        )
        .await;
    }
}

/// The wildcard matcher the comparison is built on, exercised directly.
///
/// `*` stands for any sequence of characters and `?` for exactly one, per the IAM policy
/// grammar; action names are matched case-insensitively and ARNs are not, which is why
/// the matcher takes the flag. The table spans the forms a pattern takes (literal,
/// leading, trailing and interior `*`, `?`, several wildcards, the empty pattern and
/// value) and multi-byte characters, where `?` must consume one CHARACTER and not one
/// byte.
#[test]
fn the_wildcard_matcher_follows_the_policy_grammar() {
    use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::check_no_new_access::policy::wildcard_match;

    // pattern, value, case_sensitive, expected
    let rows: &[(&str, &str, bool, bool)] = &[
        ("s3:GetObject", "s3:GetObject", true, true),
        ("s3:GetObject", "s3:PutObject", true, false),
        ("*", "s3:GetObject", true, true),
        ("*", "", true, true),
        ("", "", true, true),
        ("", "s3:GetObject", true, false),
        ("s3:*", "s3:GetObject", true, true),
        ("s3:*", "sqs:SendMessage", true, false),
        ("s3:Get*", "s3:GetObject", true, true),
        ("s3:Get*", "s3:PutObject", true, false),
        ("*Object", "s3:GetObject", true, true),
        ("*Object", "s3:GetBucket", true, false),
        ("s3:*Object*", "s3:GetObjectAcl", true, true),
        ("a*b*c", "axxbyyc", true, true),
        ("a*b*c", "axxbyy", true, false),
        ("a*", "a", true, true),
        ("*a", "ba", true, true),
        ("*a", "ab", true, false),
        ("s3:Get?bject", "s3:GetObject", true, true),
        ("s3:Get?bject", "s3:Getbject", true, false),
        ("a?c", "abc", true, true),
        ("a?c", "ac", true, false),
        // Case: action names are case-insensitive, ARNs are not.
        ("S3:GETOBJECT", "s3:getobject", false, true),
        ("S3:GETOBJECT", "s3:getobject", true, false),
        ("arn:aws:s3:::My-Bucket/*", "arn:aws:s3:::my-bucket/key", true, false),
        // Multi-byte: `?` consumes one character, not one byte.
        ("b?cket", "bücket", true, true),
        ("b?cket", "bucket", true, true),
        ("bü*", "bücket", true, true),
        ("*ü*", "abücd", true, true),
        ("*ü*", "abcd", true, false),
    ];

    for (pattern, value, case_sensitive, expected) in rows {
        let actual = wildcard_match(pattern, value, *case_sensitive);
        assert_eq!(
            actual, *expected,
            "case ({pattern:?}, {value:?}, case_sensitive={case_sensitive}): expected {expected}, got {actual}"
        );
    }
}
