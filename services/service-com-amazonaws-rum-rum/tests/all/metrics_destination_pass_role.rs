// SPDX-License-Identifier: CC0-1.0

//! The role-passing permission `PutRumMetricsDestination` requires, end to end.
//!
//! The rule under test: a request that names an IAM role this service will ASSUME is
//! allowed only when the caller also holds `iam:PassRole` for that very role — the
//! identity-based decision taken against the ARN the request names — and the decision is
//! taken before anything is written, so a refused request stores no destination and no
//! role. A request that names no role to assume needs no such permission.
//!
//! Every row grants the caller this service's own `rum:PutRumMetricsDestination`, so the
//! only thing that varies is the role-passing permission. The rows span what the rule
//! generalises over: whether a role is passed at all, how the permission is withheld (not
//! granted, scoped to another role, explicitly denied) or held (every resource, a service
//! wildcard, that one role's ARN exactly), the FORM of the ARN passed (a role at the
//! default path and one under an IAM path), and the caller being the account's own
//! credential, which holds no IAM policies.

use serde_json::{json, Value};

use crate::authorization::{
    account, identity_policy, role_arn, statement, Emulator, ACCOUNT_KEY, REGION,
};

/// The wire error code, and the status, a request IAM does not allow is refused with.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// Where the caller's `iam:PassRole` permission stands for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassRole {
    /// The caller holds no IAM statement at all beside the RUM grant.
    NotGranted,
    /// `iam:PassRole` allowed on every resource.
    OnEveryRole,
    /// `iam:PassRole` allowed on exactly the role ARN the request names.
    OnThatRole,
    /// `iam:PassRole` allowed on a DIFFERENT role's ARN.
    OnAnotherRole,
    /// Allowed on every resource and explicitly denied on the role the request names.
    DeniedOnThatRole,
    /// Allowed through the IAM service wildcard `iam:*`.
    ByServiceWildcard,
    /// The caller is the account's own published credential, which holds no IAM policies.
    AccountCredential,
}

/// The inline policy a row's calling role holds: the RUM action the operation itself is
/// authorized against, plus whatever the row says about `iam:PassRole` for `passed`.
fn caller_policy(grant: PassRole, passed: &str) -> String {
    let rum = statement("Allow", "rum:PutRumMetricsDestination", "*");
    let extra = match grant {
        PassRole::NotGranted | PassRole::AccountCredential => None,
        PassRole::OnEveryRole => Some(statement("Allow", "iam:PassRole", "*")),
        PassRole::OnThatRole => Some(statement("Allow", "iam:PassRole", passed)),
        PassRole::OnAnotherRole => {
            Some(statement("Allow", "iam:PassRole", &role_arn("some-other-role")))
        }
        PassRole::DeniedOnThatRole => Some(format!(
            "{},{}",
            statement("Allow", "iam:PassRole", "*"),
            statement("Deny", "iam:PassRole", passed)
        )),
        PassRole::ByServiceWildcard => Some(statement("Allow", "iam:*", "*")),
    };
    match extra {
        Some(extra) => identity_policy(&format!("{rum},{extra}")),
        None => identity_policy(&rum),
    }
}

/// One row: what the put asks for, and whether it must be allowed.
struct Row {
    case: &'static str,
    /// The `Destination` the request names.
    destination: &'static str,
    /// The role ARN the request passes, when it passes one. `None` is a destination that
    /// names no role for RUM to assume.
    passed_role: Option<String>,
    grant: PassRole,
    allowed: bool,
}

/// The ARN of an emulated Evidently experiment, the resource an `Evidently` destination
/// names.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The `Destinations` list `ListRumMetricsDestinations` reports for `monitor`, read with
/// the account's own credential so the read is never the thing being refused.
async fn destinations(emulator: &Emulator, case: &str, monitor: &str) -> Vec<Value> {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "GET",
            &format!("/rummetrics/{monitor}/metricsdestination"),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["Destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn passing_a_role_to_a_metrics_destination_requires_the_callers_iam_pass_role() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a caller allowed the RUM action but no role-passing permission",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::NotGranted,
            allowed: false,
        },
        Row {
            case: "a caller allowed iam:PassRole on every resource",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::OnEveryRole,
            allowed: true,
        },
        Row {
            case: "a caller allowed iam:PassRole on exactly the role it passes",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::OnThatRole,
            allowed: true,
        },
        Row {
            case: "a caller allowed iam:PassRole on another role",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::OnAnotherRole,
            allowed: false,
        },
        Row {
            case: "a caller allowed iam:PassRole everywhere and denied it on that role",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::DeniedOnThatRole,
            allowed: false,
        },
        Row {
            case: "a caller holding the iam service wildcard",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::ByServiceWildcard,
            allowed: true,
        },
        Row {
            // The decision is taken against the ARN the request names, whatever its form:
            // an IAM role ARN may carry a path (`role/<path>/<name>`).
            case: "a role under an IAM path, allowed on exactly that ARN",
            destination: "Evidently",
            passed_role: Some(format!(
                "arn:aws:iam::{}:role/service-roles/rum/evidently-writer",
                account()
            )),
            grant: PassRole::OnThatRole,
            allowed: true,
        },
        Row {
            case: "a role under an IAM path, with no role-passing permission",
            destination: "Evidently",
            passed_role: Some(format!(
                "arn:aws:iam::{}:role/service-roles/rum/evidently-writer",
                account()
            )),
            grant: PassRole::NotGranted,
            allowed: false,
        },
        Row {
            // Nothing is passed to RUM, so there is no role-passing permission to hold.
            case: "a CloudWatch destination, which names no role to assume",
            destination: "CloudWatch",
            passed_role: None,
            grant: PassRole::NotGranted,
            allowed: true,
        },
        Row {
            case: "the account's own credential, which holds no IAM policies",
            destination: "Evidently",
            passed_role: Some(role_arn("rum-evidently-writer")),
            grant: PassRole::AccountCredential,
            allowed: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("pass-role-{index}");
        emulator.create_monitor(&monitor).await;

        let caller_role = format!("pass-role-caller-{index}");
        let key = if row.grant == PassRole::AccountCredential {
            ACCOUNT_KEY.to_string()
        } else {
            let policy = caller_policy(
                row.grant,
                row.passed_role.as_deref().unwrap_or_default(),
            );
            emulator.assumed_role_key(&caller_role, Some(&policy)).await
        };

        let mut body = json!({"Destination": row.destination});
        if let Some(passed) = &row.passed_role {
            body["DestinationArn"] = json!(experiment_arn(&format!("exp-{index}")));
            body["IamRoleArn"] = json!(passed);
        }
        let path = format!("/rummetrics/{monitor}/metricsdestination");
        let reply = emulator.rum(&key, "POST", &path, Some(body)).await;

        let case = row.case;
        if row.allowed {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status, ACCESS_DENIED_STATUS,
                "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            // The refusal is worded about the decision that was taken: the principal, the
            // `iam:PassRole` action and the role ARN the request asked to pass, with the
            // policy kind that withheld the grant.
            let reason = if row.grant == PassRole::DeniedOnThatRole {
                "with an explicit deny in an identity-based policy".to_string()
            } else {
                "because no identity-based policy allows the iam:PassRole action".to_string()
            };
            let expected = format!(
                "User: arn:aws:sts::{}:assumed-role/{caller_role}/probe is not authorized \
                 to perform: iam:PassRole on resource: {} {reason}",
                account(),
                row.passed_role.clone().unwrap_or_default()
            );
            assert_eq!(
                reply.message(case),
                expected,
                "case {case:?}: expected the refusal to name the role it would have \
                 passed, got {:?}",
                reply.message(case)
            );
        }

        // What the store holds now: the destination exists exactly where the put was
        // allowed, and a refused put leaves the monitor with none.
        let listed = destinations(&emulator, case, &monitor).await;
        if !row.allowed {
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused put must store no destination, got {listed:?}"
            );
            continue;
        }
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: an allowed put must store exactly one destination, got \
             {listed:?}"
        );
        // The round trip: the destination reads back with the role the request passed.
        let stored = &listed[0];
        assert_eq!(
            stored["Destination"].as_str(),
            Some(row.destination),
            "case {case:?}: expected the stored Destination to be {:?}, got {stored:?}",
            row.destination
        );
        let expected_role = row.passed_role.clone().unwrap_or_default();
        assert_eq!(
            stored["IamRoleArn"].as_str().unwrap_or_default(),
            expected_role,
            "case {case:?}: expected the stored IamRoleArn to be {expected_role:?}, got \
             {stored:?}"
        );
    }
}

/// The permission is decided per call, from the policies in force at that instant: the
/// same caller is refused, then allowed once its role-passing grant is in place — and the
/// refused call left nothing behind for the allowed one to collide with.
#[tokio::test]
async fn the_role_passing_decision_is_taken_per_call() {
    let emulator = Emulator::new();
    let monitor = "pass-role-lifecycle";
    emulator.create_monitor(monitor).await;
    let passed = role_arn("rum-evidently-writer");

    // One role holds only the RUM action; a second holds the role-passing grant too. The
    // put is identical, so the decision is the only difference between the two calls.
    let without = emulator
        .assumed_role_key(
            "pass-role-without",
            Some(&caller_policy(PassRole::NotGranted, &passed)),
        )
        .await;
    let with = emulator
        .assumed_role_key(
            "pass-role-with",
            Some(&caller_policy(PassRole::OnThatRole, &passed)),
        )
        .await;

    let path = format!("/rummetrics/{monitor}/metricsdestination");
    let body = json!({
        "Destination": "Evidently",
        "DestinationArn": experiment_arn("lifecycle"),
        "IamRoleArn": passed,
    });

    let case = "the caller without the role-passing grant";
    let reply = emulator
        .rum(&without, "POST", &path, Some(body.clone()))
        .await;
    assert_eq!(
        reply.status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        destinations(&emulator, case, monitor).await.is_empty(),
        "case {case:?}: the refused put must have stored nothing, got {:?}",
        destinations(&emulator, case, monitor).await
    );

    let case = "the caller holding the role-passing grant";
    let reply = emulator.rum(&with, "POST", &path, Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let listed = destinations(&emulator, case, monitor).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected one stored destination, got {listed:?}"
    );
    assert_eq!(
        listed[0]["IamRoleArn"].as_str(),
        Some(passed.as_str()),
        "case {case:?}: expected the stored role to be the one passed, got {:?}",
        listed[0]
    );
}
