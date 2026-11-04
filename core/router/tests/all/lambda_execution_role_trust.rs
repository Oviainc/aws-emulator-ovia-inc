// SPDX-License-Identifier: CC0-1.0

//! Lambda's execution-role check, across the two services it spans.
//!
//! RULE: an operation that STORES the execution role a function runs as resolves that role
//! against this account's IAM records first and refuses the write with
//! `InvalidParameterValueException` (HTTP 400) — persisting nothing — when IAM reports a role
//! whose trust policy does not let `lambda.amazonaws.com` perform `sts:AssumeRole` on it.
//! Only a role this account's IAM HOLDS can be refused for its trust: a name it holds nothing
//! under is no evidence about the role the caller named, since an execution role is normally
//! provisioned outside the client that creates the function. Both write paths ask:
//! `CreateFunction` for the role a new function is created with, and
//! `UpdateFunctionConfiguration` for a role that replaces it.
//!
//! The role is resolved against the ACCOUNT's IAM records, which is why this suite lives
//! here: each role is created through the real `CreateRole` on the same engine that serves
//! Lambda, so what the check reads is the document IAM persisted and not a copy.
//!
//! The rows differ along the form of the trust policy — which service principal it names,
//! whether it names several, which statement carries the grant, whether the grant is an
//! `Allow`, whether an explicit `Deny` stands beside it, which action the statement names,
//! which wildcard stands in the action's place, and which KIND of principal it names — and,
//! in the later tables, along whether this account's IAM holds the named role at all, which
//! path it lives under, which characters its name holds, and which account and partition the
//! ARN names.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder an `application/x-www-form-urlencoded` component needs is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns, shared rather than copied.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;
use crate::lambda_signing_name_routing::{AUTHORIZATION, REGION, ZIP_FILE};

/// The service principal a function's execution role has to admit.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// AWS's refusal of a role the service cannot assume, which both write paths answer with.
const NOT_ASSUMABLE: &str = "The role defined for the function cannot be assumed by Lambda.";

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them. The engine is handed back too, so a row can bring a function's
/// clock-owed activation forward instead of sleeping for it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
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

/// One restJson1 call to the Lambda control plane, answered with the HTTP status, the wire
/// error code (`""` on a success) and the JSON body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    (status, code, parsed)
}

/// The `CreateFunction` request body for a function of `name` running as `role`.
fn create_body(name: &str, role: &str) -> Value {
    json!({
        "FunctionName": name,
        "Role": role,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": ZIP_FILE },
    })
}

/// Create the role `name` with the trust document `document`, and report its ARN.
async fn create_role(router: &MultiServiceRouter, case: &str, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRole to persist the role, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// Move every stored function's activation and in-flight-update instants into the past, so
/// the next call materialises the `Pending` -> `Active` and update-finished transitions the
/// clock owes. This is what waiting would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// A trust policy with one statement naming `principals` under `effect` for `action`.
fn trust(effect: &str, action: Value, principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": action
        }]
    })
    .to_string()
}

/// One row: a trust policy, and whether a function created under a role carrying it is
/// stored.
struct Row {
    case: &'static str,
    document: String,
    accepted: bool,
}

/// The table the rule is asserted over: the forms a trust policy takes, and whether each
/// of them lets this service assume the role.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "lambda alone, named as a string",
            document: trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            ),
            accepted: true,
        },
        Row {
            case: "lambda among several services, named as a list",
            document: trust(
                "Allow",
                json!(["sts:TagSession", "sts:AssumeRole"]),
                json!({ "Service": ["ec2.amazonaws.com", LAMBDA_PRINCIPAL] }),
            ),
            accepted: true,
        },
        Row {
            case: "lambda granted by the second statement",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "ec2.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": LAMBDA_PRINCIPAL },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            accepted: true,
        },
        Row {
            case: "lambda allowed through an sts wildcard",
            document: trust(
                "Allow",
                json!("sts:*"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            ),
            accepted: true,
        },
        Row {
            case: "lambda allowed every action",
            document: trust("Allow", json!("*"), json!({ "Service": LAMBDA_PRINCIPAL })),
            accepted: true,
        },
        Row {
            case: "lambda named in another casing",
            document: trust(
                "Allow",
                json!("STS:AssumeRole"),
                json!({ "Service": "Lambda.AmazonAWS.com" }),
            ),
            accepted: true,
        },
        Row {
            // The recorded negative: a role trusting only the account root.
            case: "the account root alone",
            document: trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "AWS": "arn:aws:iam::000000000000:root" }),
            ),
            accepted: false,
        },
        Row {
            // The other recorded negative: a role trusting a different service.
            case: "another service's principal",
            document: trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": "ec2.amazonaws.com" }),
            ),
            accepted: false,
        },
        Row {
            case: "lambda named by a statement that denies it",
            document: trust(
                "Deny",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            ),
            accepted: false,
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over any `Allow` beside it.
            case: "lambda both allowed and denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": LAMBDA_PRINCIPAL },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": LAMBDA_PRINCIPAL },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            // A trust policy governs `sts:AssumeRole`; naming lambda for some other
            // action grants it nothing.
            case: "lambda allowed an action that is not assuming the role",
            document: trust(
                "Allow",
                json!("s3:GetObject"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            ),
            accepted: false,
        },
        Row {
            // A policy that carries no statement grants nothing, so it admits nobody.
            case: "a document carrying an empty statement list",
            document: json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn create_function_refuses_a_role_whose_trust_policy_withholds_lambda() {
    let (router, _engine) = emulator();

    for (index, row) in rows().iter().enumerate() {
        let role_arn = create_role(
            &router,
            row.case,
            &format!("lambda-trust-{index}"),
            &row.document,
        )
        .await;
        let name = format!("trust-probe-{index}");

        let (status, code, created) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            Some(create_body(&name, &role_arn)),
        )
        .await;

        if row.accepted {
            assert_eq!(
                (status, code.as_str()),
                (201, ""),
                "case {:?}: a role that lets lambda assume it must create the function, \
                 got {status} {code:?}: {created}",
                row.case
            );
            assert_eq!(
                created["Role"].as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the function runs as the role the request named, got {created}",
                row.case
            );
        } else {
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {:?}: a role that withholds the grant must be refused, \
                 got {status} {code:?}: {created}",
                row.case
            );
            assert_eq!(
                created["message"].as_str(),
                Some(NOT_ASSUMABLE),
                "case {:?}: expected the modelled refusal message, got {created}",
                row.case
            );
        }

        // Accepted or refused, the store holds exactly what the answer claimed: a refused
        // create persists NOTHING, so the configuration read reports the function only
        // for the rows that were served.
        let (status, code, fetched) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        let expected = if row.accepted {
            (200, "")
        } else {
            (404, "ResourceNotFoundException")
        };
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {:?}: expected HTTP {} {:?} reading the function back, got {status} \
             {code:?}: {fetched}",
            row.case,
            expected.0,
            expected.1
        );
    }
}

#[tokio::test]
async fn update_function_configuration_refuses_a_role_whose_trust_policy_withholds_lambda() {
    let (router, engine) = emulator();

    // One function, created under a role that does admit this service, is then asked to
    // run as each row's role in turn.
    let admitting = create_role(
        &router,
        "the function's first role",
        "lambda-update-base",
        &trust(
            "Allow",
            json!("sts:AssumeRole"),
            json!({ "Service": LAMBDA_PRINCIPAL }),
        ),
    )
    .await;
    let name = "trust-update-probe";
    let (status, code, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        Some(create_body(name, &admitting)),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (201, ""),
        "case \"the function's first role\": expected the function to be created, \
         got {status} {code:?}: {created}"
    );

    for (index, row) in rows().iter().enumerate() {
        // "During this time, you can't invoke or modify the function": bring the
        // activation the clock owes forward so each row's update is judged on its role.
        make_activation_due(&engine);

        let role_arn = create_role(
            &router,
            row.case,
            &format!("lambda-update-trust-{index}"),
            &row.document,
        )
        .await;
        let (status, code, updated) = lambda(
            &router,
            "PUT",
            &format!("/2015-03-31/functions/{name}/configuration"),
            Some(json!({ "Role": role_arn })),
        )
        .await;

        if row.accepted {
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {:?}: a role that lets lambda assume it must be stored, \
                 got {status} {code:?}: {updated}",
                row.case
            );
            assert_eq!(
                updated["Role"].as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the function must run as the role the update named, got {updated}",
                row.case
            );
        } else {
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {:?}: a role that withholds the grant must be refused, \
                 got {status} {code:?}: {updated}",
                row.case
            );
            assert_eq!(
                updated["message"].as_str(),
                Some(NOT_ASSUMABLE),
                "case {:?}: expected the modelled refusal message, got {updated}",
                row.case
            );
        }

        // A refused update changes NOTHING: the stored role is still the last one that
        // was accepted.
        make_activation_due(&engine);
        let (status, code, fetched) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {:?}: expected the configuration to read back, got {status} {code:?}: \
             {fetched}",
            row.case
        );
        let expected_role = if row.accepted {
            role_arn.as_str()
        } else {
            admitting.as_str()
        };
        assert_eq!(
            fetched["Role"].as_str(),
            Some(expected_role),
            "case {:?}: expected the stored role to be {expected_role}, got {fetched}",
            row.case
        );

        // Leave the function on a role that admits this service, so the next row starts
        // from the same place whichever way this one went.
        if row.accepted {
            make_activation_due(&engine);
            let (status, code, restored) = lambda(
                &router,
                "PUT",
                &format!("/2015-03-31/functions/{name}/configuration"),
                Some(json!({ "Role": admitting })),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {:?}: expected the role to be restored, got {status} {code:?}: \
                 {restored}",
                row.case
            );
        }
    }
}

/// RULE (the complement): the refusal is grounded in a TRUST DOCUMENT IAM answered with, so
/// only a role this account's IAM HOLDS can be refused for one. A role name this account's
/// IAM holds nothing under is no evidence about the role the caller named — an execution role
/// is normally provisioned outside the client that creates the function, so this emulator's
/// IAM holds a record of it only when that client created it here — and neither is an ARN
/// naming ANOTHER account's role, which is not this emulator's IAM to ask about. Both write
/// paths serve those, and store the role as supplied.
///
/// The rows differ along which IAM the ARN names and what it holds: this account's, held
/// (admitting and withholding) and unheld (at the default path, under a path, and with the
/// punctuation a role name admits), and another account's, in this partition and in another.
fn unheld_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "a role this account's IAM does not hold",
            "arn:aws:iam::000000000000:role/never-created".to_string(),
            true,
        ),
        (
            "a role under a path this account's IAM does not hold",
            "arn:aws:iam::000000000000:role/service-role/never-created".to_string(),
            true,
        ),
        (
            // The characters IAM admits in a role name, per the model's `RoleArn` pattern
            // (`smithy_query(query="shape", shape="RoleArn")`).
            "an unheld role name holding every admitted character class",
            "arn:aws:iam::000000000000:role/a_b-1+2=3,4.5@6".to_string(),
            true,
        ),
        (
            "another account's role",
            "arn:aws:iam::123456789012:role/remote-exec".to_string(),
            true,
        ),
        (
            "another account's role under a path",
            "arn:aws:iam::123456789012:role/service-role/remote-exec".to_string(),
            true,
        ),
        (
            "another partition's role in another account",
            "arn:aws-us-gov:iam::123456789012:role/remote-exec".to_string(),
            true,
        ),
    ]
}

#[tokio::test]
async fn create_function_refuses_only_a_role_this_accounts_iam_holds() {
    let (router, _engine) = emulator();

    // The held rows are created through the real `CreateRole` on the same engine, so the
    // negative row is refused on the document IAM persisted for it and not on a copy.
    let mut rows = unheld_rows();
    rows.push((
        "a role this account's IAM holds, whose trust policy admits this service",
        create_role(
            &router,
            "a role this account's IAM holds, whose trust policy admits this service",
            "lambda-held-admitting",
            &trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": LAMBDA_PRINCIPAL }),
            ),
        )
        .await,
        true,
    ));
    rows.push((
        "a role this account's IAM holds, whose trust policy withholds this service",
        create_role(
            &router,
            "a role this account's IAM holds, whose trust policy withholds this service",
            "lambda-held-withholding",
            &trust(
                "Allow",
                json!("sts:AssumeRole"),
                json!({ "Service": "ec2.amazonaws.com" }),
            ),
        )
        .await,
        false,
    ));

    for (index, (case, role_arn, accepted)) in rows.iter().enumerate() {
        let name = format!("unheld-probe-{index}");
        let (status, code, created) = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            Some(create_body(&name, role_arn)),
        )
        .await;
        if *accepted {
            assert_eq!(
                (status, code.as_str()),
                (201, ""),
                "case {case:?}: a role no trust document of this account's IAM withholds \
                 must be accepted, got {status} {code:?}: {created}"
            );
            assert_eq!(
                created["Role"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the function runs as the role the request named, got {created}"
            );
        } else {
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: a role whose stored trust policy withholds this service must \
                 be refused, got {status} {code:?}: {created}"
            );
            assert_eq!(
                created["message"].as_str(),
                Some(NOT_ASSUMABLE),
                "case {case:?}: expected the modelled refusal message, got {created}"
            );
        }

        // The answer and the store agree: a refused create persists nothing.
        let (status, code, fetched) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        let expected = if *accepted {
            (200, "")
        } else {
            (404, "ResourceNotFoundException")
        };
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected HTTP {} {:?} reading the function back, got {status} \
             {code:?}: {fetched}",
            expected.0,
            expected.1
        );
        if *accepted {
            assert_eq!(
                fetched["Role"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: expected the stored role to be {role_arn}, got {fetched}"
            );
        }
    }
}

/// RULE (the same, on the OTHER write path): `UpdateFunctionConfiguration` stores a role no
/// trust document of this account's IAM withholds, and the replacement is what a later read
/// reports.
#[tokio::test]
async fn update_function_configuration_stores_a_role_this_accounts_iam_holds_nothing_under() {
    let (router, engine) = emulator();

    let admitting = create_role(
        &router,
        "the function's first role",
        "lambda-unheld-update-base",
        &trust(
            "Allow",
            json!("sts:AssumeRole"),
            json!({ "Service": LAMBDA_PRINCIPAL }),
        ),
    )
    .await;
    let name = "unheld-update-probe";
    let (status, code, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        Some(create_body(name, &admitting)),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (201, ""),
        "case \"the function's first role\": expected the function to be created, \
         got {status} {code:?}: {created}"
    );

    for (case, role_arn, _) in unheld_rows() {
        // "During this time, you can't invoke or modify the function": bring the clock-owed
        // transitions forward so each row's update is judged on its role.
        make_activation_due(&engine);
        let (status, code, updated) = lambda(
            &router,
            "PUT",
            &format!("/2015-03-31/functions/{name}/configuration"),
            Some(json!({ "Role": role_arn })),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: a role no trust document of this account's IAM withholds must be \
             stored, got {status} {code:?}: {updated}"
        );

        make_activation_due(&engine);
        let (status, code, fetched) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: expected the configuration to read back, got {status} {code:?}: \
             {fetched}"
        );
        assert_eq!(
            fetched["Role"].as_str(),
            Some(role_arn.as_str()),
            "case {case:?}: expected the stored role to be {role_arn}, got {fetched}"
        );
    }
}
