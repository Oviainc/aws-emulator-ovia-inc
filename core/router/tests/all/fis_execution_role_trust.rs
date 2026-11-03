// SPDX-License-Identifier: CC0-1.0

//! FIS's execution-role check, across the two services it spans.
//!
//! RULE: `StartExperiment` runs the experiment as the template's `roleArn`, so it checks
//! up front that the role's trust policy lets `fis.amazonaws.com` assume it and refuses
//! the start with `ValidationException` (HTTP 400) — creating no experiment — when it does
//! not. The role is resolved against the ACCOUNT's IAM records, which is why this suite
//! lives here: the role is created through the real `CreateRole` on the same engine that
//! serves FIS, so what the check reads is the document IAM persisted and not a copy.
//!
//! The rows differ along the form of the trust policy (which service principal it names,
//! whether it names several, which statement carries the grant, whether the grant is an
//! `Allow`, whether an explicit `Deny` stands beside it, which action the statement names,
//! and which kind of principal it names) and along whether the IAM store holds the role at
//! all — a role ARN it does not hold is no evidence of a withheld trust, so the start goes
//! ahead.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder this suite needs is the one `all/cloudfront_key_value_store_handshake.rs`
// already owns: an `application/x-www-form-urlencoded` component is escaped exactly as a
// URI path segment is, so the codec is shared rather than written twice.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// A SigV4 credential scope naming fis, which is what routes a request to that service.
const FIS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/fis/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// A wait inside the 1..720 minutes FIS documents for `aws:fis:wait`, and long enough that
/// an experiment that starts is still running when the row reads it back.
const WAIT_DURATION: &str = "PT5M";

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// The text of the first `<tag>` element of an XML body.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"))
        + start;
    body[start..end].to_string()
}

/// One restJson1 call to FIS, answered with the HTTP status and the JSON body.
async fn fis(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", FIS_AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A trust policy naming `principals` in one `Allow` statement.
fn trust_allowing(principals: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principals },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// One row: a trust policy, and whether an experiment under a role carrying it starts.
struct Row {
    case: &'static str,
    document: String,
    starts: bool,
}

#[tokio::test]
async fn start_experiment_refuses_a_role_whose_trust_policy_withholds_fis() {
    let router = emulator();

    let rows = [
        Row {
            case: "fis alone",
            document: trust_allowing(&["fis.amazonaws.com"]),
            starts: true,
        },
        Row {
            case: "fis among several services",
            document: trust_allowing(&["ec2.amazonaws.com", "fis.amazonaws.com"]),
            starts: true,
        },
        Row {
            case: "fis named as a single string rather than a list",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "fis.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            starts: true,
        },
        Row {
            case: "fis granted by the second statement",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "fis.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            starts: true,
        },
        Row {
            case: "ec2 only",
            document: trust_allowing(&["ec2.amazonaws.com"]),
            starts: false,
        },
        Row {
            case: "lambda only",
            document: trust_allowing(&["lambda.amazonaws.com"]),
            starts: false,
        },
        Row {
            case: "fis named by a statement that denies it",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "Service": "fis.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            starts: false,
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over any `Allow`, whichever
            // order the two statements are written in.
            case: "fis both allowed and denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "fis.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": "fis.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            starts: false,
        },
        Row {
            // The statement names fis, but for an action that is not assuming the role, so
            // it grants nothing: a trust policy governs `sts:AssumeRole`.
            case: "fis allowed an action that is not assuming the role",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "fis.amazonaws.com" },
                    "Action": "s3:GetObject"
                }]
            })
            .to_string(),
            starts: false,
        },
        Row {
            // A wildcard IAM admits in the action's place still covers assuming the role.
            case: "fis allowed through an sts wildcard",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "fis.amazonaws.com" },
                    "Action": "sts:*"
                }]
            })
            .to_string(),
            starts: true,
        },
        Row {
            case: "an account principal rather than a service one",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            starts: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let role_name = format!("fis-trust-{index}");
        let (status, created) = iam(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", row.document.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateRole to persist the role, got {status}: {created}",
            row.case
        );
        let role_arn = element(&created, "Arn");

        let (status, template) = fis(
            &router,
            "POST",
            "/experimentTemplates",
            Some(json!({
                "clientToken": format!("trust-template-{index}"),
                "description": format!("role trust: {}", row.case),
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                // `targets` is a member FIS requires of a create request, so every body
                // here names it.
                "targets": {},
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": WAIT_DURATION }
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the template to be created, got {status}: {template}",
            row.case
        );
        let template_id = template["experimentTemplate"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, started) = fis(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("trust-start-{index}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;

        if row.starts {
            assert_eq!(
                status, 200,
                "case {:?}: a role that grants fis must start, got {status}: {started}",
                row.case
            );
            assert_eq!(
                started["experiment"]["roleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the experiment runs as the template's role, got {started}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a role that withholds the grant must be refused, \
                 got {status}: {started}",
                row.case
            );
            assert_eq!(
                started["__type"].as_str(),
                Some("ValidationException"),
                "case {:?}: expected ValidationException, got {started}",
                row.case
            );
            let message = started["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&role_arn) && message.contains("fis.amazonaws.com"),
                "case {:?}: the refusal names the role and the principal it needs, \
                 got {message:?}",
                row.case
            );
        }

        // A refused start creates NOTHING: the template has no experiment at all.
        let (status, listed) = fis(
            &router,
            "GET",
            &format!("/experiments?experimentTemplateId={template_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListExperiments to answer, got {status}: {listed}",
            row.case
        );
        let count = listed["experiments"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count,
            usize::from(row.starts),
            "case {:?}: expected {} experiment(s) for that template, got {listed}",
            row.case,
            usize::from(row.starts)
        );

        // This row is done with the experiment it started: stop it, so the suite holds no
        // active-experiment slot per row and no later row can be refused for having
        // reached the account's quota.
        if let Some(id) = started["experiment"]["id"].as_str() {
            let (status, stopped) = fis(&router, "DELETE", &format!("/experiments/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the stop to be accepted, got {status}: {stopped}",
                row.case
            );
        }
    }
}

/// RULE (the complement): a `roleArn` the account's IAM store holds no role for is not
/// evidence that the trust is missing — the role may live in another account, or in an IAM
/// this emulator was never told about — so the start goes ahead.
#[tokio::test]
async fn start_experiment_accepts_a_role_arn_the_iam_store_does_not_hold() {
    let router = emulator();

    // The rows differ along the shape of the unheld ARN: this account's own namespace, a
    // role with a path, another account, and another partition.
    let rows = [
        ("this account", "arn:aws:iam::000000000000:role/not-created"),
        (
            "a role with a path",
            "arn:aws:iam::000000000000:role/team/not-created",
        ),
        ("another account", "arn:aws:iam::123456789012:role/remote"),
        (
            "another partition",
            "arn:aws-us-gov:iam::000000000000:role/remote",
        ),
    ];

    for (index, (case, role_arn)) in rows.iter().enumerate() {
        let (status, template) = fis(
            &router,
            "POST",
            "/experimentTemplates",
            Some(json!({
                "clientToken": format!("unheld-template-{index}"),
                "description": format!("role not in iam: {case}"),
                "roleArn": role_arn,
                "stopConditions": [{ "source": "none" }],
                // `targets` is a member FIS requires of a create request, so every body
                // here names it.
                "targets": {},
                "actions": {
                    "wait": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": WAIT_DURATION }
                    }
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the template to be created, got {status}: {template}"
        );
        let template_id = template["experimentTemplate"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, started) = fis(
            &router,
            "POST",
            "/experiments",
            Some(json!({
                "clientToken": format!("unheld-start-{index}"),
                "experimentTemplateId": template_id
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the start to go ahead, got {status}: {started}"
        );
        assert_eq!(
            started["experiment"]["roleArn"].as_str(),
            Some(*role_arn),
            "case {case:?}: the experiment runs as the template's role, got {started}"
        );

        // Stop it for the same reason the table above does: the suite must not depend on
        // how many active-experiment slots its rows happen to occupy.
        let id = started["experiment"]["id"].as_str().unwrap_or_default();
        let (status, stopped) = fis(&router, "DELETE", &format!("/experiments/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop to be accepted, got {status}: {stopped}"
        );
    }
}
