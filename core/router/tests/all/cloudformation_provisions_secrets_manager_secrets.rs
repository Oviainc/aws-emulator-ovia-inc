// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack's `AWS::SecretsManager::Secret` is a secret of AWS Secrets
//! Manager, under the ARN the stack publishes for it.
//!
//! The rule under test: **a resource of a type this emulator provisions is CREATED in the
//! service that owns it, and the identifier the stack publishes for the resource is the one
//! that service minted — so every call a client makes with that identifier resolves there.**
//! A secret's ARN ends in six characters Secrets Manager draws per secret, so nothing
//! outside that service can compute it: the ARN a `Ref` answers has to be the one the create
//! came back with. Two crates have to agree for the rule to hold (CloudFormation makes the
//! call, Secrets Manager serves it), so the suite lives here rather than in either of them.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! secret or leaves the name to CloudFormation, which of the two value forms the declaration
//! states (`SecretString` verbatim, `GenerateSecretString` on its own, or
//! `GenerateSecretString` merged into a `SecretStringTemplate` under a `GenerateStringKey`),
//! whether it carries a description, its own tags and replica Regions, and which intrinsic
//! each output is written with (`Ref` and `Fn::GetAtt`). Every expected value is computed
//! here from what the row declares, never read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the Region the stack's secret and its
/// ARN belong to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How many characters a Secrets Manager secret's ARN ends in, which that service draws per
/// secret (`arn:aws:secretsmanager:<region>:<account>:secret:<name>-<suffix>`).
const ARN_SUFFIX_CHARACTERS: usize = 6;

/// How long one `DescribeStacks` poll waits before the next, and how many are made: a
/// stack's own status transition is owed on its clock, so the suite waits for the status
/// rather than for a span.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 120;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one AWS Secrets Manager operation over its awsJson1_1 wire.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization("secretsmanager"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Create a stack from `template` with `tags` as its stack-level tags, and answer its id.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut members: Vec<(String, String)> = vec![
        ("StackName".to_string(), stack_name.to_string()),
        ("TemplateBody".to_string(), template.to_string()),
    ];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        members.push((format!("Tags.member.{position}.Key"), (*key).to_string()));
        members.push((format!("Tags.member.{position}.Value"), (*value).to_string()));
    }
    let borrowed: Vec<(&str, &str)> = members
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = cloudformation(router, "CreateStack", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    crate::query_wire::opt_element(&body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// Poll `stack` until it first reports `wanted`, and answer the last status with the
/// `DescribeStacks` answer it was read from. The transition is owed on the stack's own clock,
/// so nothing is waited out past the first observation.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    for _ in 0..POLLS {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = crate::query_wire::opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == wanted {
            return (reported, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (_, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    (
        crate::query_wire::opt_element(&body, "StackStatus").unwrap_or_default(),
        body,
    )
}

/// The value a row expects `GetSecretValue` to answer with for the secret the stack
/// declared.
enum ExpectedValue {
    /// The declaration stated the value verbatim, so that exact string is the value.
    Verbatim(&'static str),
    /// The declaration asked the owning service to generate one: the value is a password of
    /// the declared length, which the suite can only state the SHAPE of.
    GeneratedPassword { length: usize },
    /// The declaration asked for a generated password merged into a JSON document: every
    /// member the template stated is there unchanged, and the declared key carries a
    /// generated password of the declared length.
    GeneratedInDocument {
        members: &'static [(&'static str, &'static str)],
        key: &'static str,
        length: usize,
    },
}

/// One row of the table: a declaration, and what the rule says the stack must then publish
/// and Secrets Manager must then hold.
struct Row {
    case: &'static str,
    /// The secret's `Properties`, built from the stack name the row runs under.
    properties: fn(&str) -> Value,
    /// The NAME the secret must end up carrying, `None` where the declaration states none
    /// and CloudFormation generates it.
    expected_name: fn(&str) -> Option<String>,
    /// The value `GetSecretValue` must answer with.
    expected_value: ExpectedValue,
    /// The `Description` `DescribeSecret` must report, `None` where the declaration states
    /// none.
    expected_description: Option<&'static str>,
    /// The tags the secret must carry beside the stack-level ones this suite propagates.
    expected_own_tags: &'static [(&'static str, &'static str)],
}

/// The logical id every row declares its secret under, which is also part of the name
/// CloudFormation generates for a declaration that names none.
const SECRET_LOGICAL_ID: &str = "AppSecret";

/// The stack-level tags every row's stack is created with: one of a client's own, which the
/// secret must carry, beside the three `aws:cloudformation:*` tags CloudFormation adds
/// itself, which Secrets Manager refuses from a caller and which therefore reach no secret.
const STACK_TAGS: &[(&str, &str)] = &[("Owner", "platform")];

const ROWS: &[Row] = &[
    Row {
        case: "a named secret whose value the template states verbatim",
        properties: |stack| {
            json!({
                "Name": format!("{stack}-db-credentials"),
                "Description": "the application's database credentials",
                "SecretString": "{\"username\":\"admin\",\"password\":\"declared-value\"}"
            })
        },
        expected_name: |stack| Some(format!("{stack}-db-credentials")),
        expected_value: ExpectedValue::Verbatim(
            "{\"username\":\"admin\",\"password\":\"declared-value\"}",
        ),
        expected_description: Some("the application's database credentials"),
        expected_own_tags: &[],
    },
    Row {
        case: "an unnamed secret carrying its own tags",
        properties: |_| {
            json!({
                "SecretString": "a-plain-value",
                "Tags": [{"Key": "Tier", "Value": "backend"}]
            })
        },
        expected_name: |_| None,
        expected_value: ExpectedValue::Verbatim("a-plain-value"),
        expected_description: None,
        expected_own_tags: &[("Tier", "backend")],
    },
    Row {
        case: "a secret whose value the owning service generates",
        properties: |stack| {
            json!({
                "Name": format!("{stack}-generated"),
                "GenerateSecretString": {
                    "PasswordLength": 24,
                    "ExcludePunctuation": true
                }
            })
        },
        expected_name: |stack| Some(format!("{stack}-generated")),
        expected_value: ExpectedValue::GeneratedPassword { length: 24 },
        expected_description: None,
        expected_own_tags: &[],
    },
    Row {
        case: "a secret whose generated password is merged into a declared document",
        properties: |stack| {
            json!({
                "Name": format!("{stack}-merged"),
                "Description": "a credential document with a generated password",
                "GenerateSecretString": {
                    "SecretStringTemplate": "{\"username\":\"appuser\",\"engine\":\"postgres\"}",
                    "GenerateStringKey": "password",
                    "PasswordLength": 18
                }
            })
        },
        expected_name: |stack| Some(format!("{stack}-merged")),
        expected_value: ExpectedValue::GeneratedInDocument {
            members: &[("username", "appuser"), ("engine", "postgres")],
            key: "password",
            length: 18,
        },
        expected_description: Some("a credential document with a generated password"),
        expected_own_tags: &[],
    },
    Row {
        case: "a named secret replicated to another Region",
        properties: |stack| {
            json!({
                "Name": format!("{stack}-replicated"),
                "SecretString": "a-replicated-value",
                "ReplicaRegions": [{"Region": "us-east-1"}]
            })
        },
        expected_name: |stack| Some(format!("{stack}-replicated")),
        expected_value: ExpectedValue::Verbatim("a-replicated-value"),
        expected_description: None,
        expected_own_tags: &[],
    },
];

/// The template one row deploys: its secret, plus the two outputs a client reads the handle
/// off — the `Ref` and the one `Fn::GetAtt` attribute the type publishes.
fn template_for(row: &Row, stack_name: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a stack whose application reads a secret it declares",
        "Resources": {
            SECRET_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": (row.properties)(stack_name)
            }
        },
        "Outputs": {
            "SecretRef": { "Value": { "Ref": SECRET_LOGICAL_ID } },
            "SecretId": { "Value": { "Fn::GetAtt": [SECRET_LOGICAL_ID, "Id"] } }
        }
    })
    .to_string()
}

/// The name inside a secret's ARN: its last colon-separated segment without the
/// [`ARN_SUFFIX_CHARACTERS`]-character suffix and the hyphen before it.
fn name_in(arn: &str) -> String {
    let addressed = arn.rsplit(':').next().unwrap_or_default();
    match addressed.len().checked_sub(ARN_SUFFIX_CHARACTERS + 1) {
        Some(end) => addressed[..end].to_string(),
        None => addressed.to_string(),
    }
}

/// The tags `DescribeSecret` reports, as the map a client reads them as.
fn tags_of(described: &Value) -> BTreeMap<String, String> {
    described["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag["Key"].as_str()?.to_string(),
                        tag["Value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the ARN a stack publishes for an `AWS::SecretsManager::Secret` is the one Secrets
/// Manager minted, the secret really is in that service under it, and its value is the one
/// the declaration asked for.
#[tokio::test]
async fn a_stack_provisions_the_secret_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("secretstack{index}");
        let template = template_for(row, &stack_name);
        create_stack(&router, case, &stack_name, &template, STACK_TAGS).await;

        let (status, body) = wait_for_stack_status(&router, &stack_name, "CREATE_COMPLETE").await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = crate::query_wire::outputs_of(&body);
        let published = outputs
            .get("SecretRef")
            .unwrap_or_else(|| panic!("case {case:?}: no SecretRef output in {body}"))
            .clone();

        // The handle is an ARN of the form Secrets Manager builds, over the name
        // CloudFormation decided.
        let expected_prefix = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT_ID}:secret:");
        assert!(
            published.starts_with(&expected_prefix),
            "case {case:?}: expected the Ref to be a secret ARN starting {expected_prefix:?}, \
             got {published:?}"
        );
        let name = name_in(&published);
        match (row.expected_name)(&stack_name) {
            Some(expected) => assert_eq!(
                name, expected,
                "case {case:?}: expected the ARN to name the declared secret {expected:?}, got \
                 {name:?} out of {published:?}"
            ),
            None => {
                let prefix = format!("{stack_name}-{SECRET_LOGICAL_ID}-");
                assert!(
                    name.starts_with(&prefix),
                    "case {case:?}: expected a generated name starting {prefix:?}, got {name:?}"
                );
            }
        }
        // The one attribute the type publishes is that same ARN.
        assert_eq!(
            outputs.get("SecretId"),
            Some(&published),
            "case {case:?}: expected the Id attribute to be the published ARN, got {outputs:?}"
        );

        // The secret exists in Secrets Manager under that ARN, and its value is the one the
        // declaration asked for.
        let (status, read) = secretsmanager(
            &router,
            "GetSecretValue",
            &json!({ "SecretId": published }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSecretValue on the published ARN to be served, got \
             {status} with body {read}"
        );
        assert_eq!(
            read["ARN"].as_str(),
            Some(published.as_str()),
            "case {case:?}: expected the read to answer for the published ARN, got {read}"
        );
        let value = read["SecretString"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no SecretString in {read}"))
            .to_string();
        match &row.expected_value {
            ExpectedValue::Verbatim(expected) => assert_eq!(
                value, *expected,
                "case {case:?}: expected the declared value {expected:?}, got {value:?}"
            ),
            ExpectedValue::GeneratedPassword { length } => {
                assert_eq!(
                    value.chars().count(),
                    *length,
                    "case {case:?}: expected a generated password of {length} characters, got \
                     {value:?}"
                );
                assert!(
                    value.chars().all(|character| character.is_ascii_alphanumeric()),
                    "case {case:?}: expected the declared punctuation exclusion to hold, got \
                     {value:?}"
                );
            }
            ExpectedValue::GeneratedInDocument {
                members,
                key,
                length,
            } => {
                let document: Value = serde_json::from_str(&value).unwrap_or_else(|error| {
                    panic!("case {case:?}: expected a JSON document, got {value:?} ({error})")
                });
                for (member, expected) in *members {
                    assert_eq!(
                        document[*member].as_str(),
                        Some(*expected),
                        "case {case:?}: expected the template's {member} to be kept as \
                         {expected:?}, got {document}"
                    );
                }
                let generated = document[*key]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} in {document}"));
                assert_eq!(
                    generated.chars().count(),
                    *length,
                    "case {case:?}: expected a generated password of {length} characters under \
                     {key:?}, got {generated:?}"
                );
            }
        }

        // What the service RECORDS about the secret is what the declaration stated, with the
        // stack's own tags on it and none of CloudFormation's reserved ones (Secrets Manager
        // refuses a caller's `aws:` key, so no deployment may send one).
        let (status, described) = secretsmanager(
            &router,
            "DescribeSecret",
            &json!({ "SecretId": published }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSecret on the published ARN to be served, got \
             {status} with body {described}"
        );
        assert_eq!(
            described["Description"].as_str(),
            row.expected_description,
            "case {case:?}: expected the declared description, got {described}"
        );
        let carried = tags_of(&described);
        let mut expected_tags: BTreeMap<String, String> = row
            .expected_own_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        for (key, value) in STACK_TAGS {
            expected_tags.insert((*key).to_string(), (*value).to_string());
        }
        assert_eq!(
            carried, expected_tags,
            "case {case:?}: expected the declared and stack tags alone, got {carried:?}"
        );
    }
}

/// RULE, from the other side: the secret a stack reports created is in the OWNING service's
/// own namespace, so a stack declaring a name that service already holds is refused by it
/// and the stack fails rather than publishing an ARN for a secret nothing created.
#[tokio::test]
async fn a_stack_naming_a_secret_the_account_already_holds_is_refused() {
    let router = build_router();
    let case = "a template naming a secret a client already created";
    let secret_name = "already-taken-secret";

    let (status, created) = secretsmanager(
        &router,
        "CreateSecret",
        &json!({
            "Name": secret_name,
            // An SDK generates this idempotency token itself; a raw request carries its own.
            "ClientRequestToken": "0d3f5f2b-9b4e-4a7d-8f0c-1e5a6c7d8e90",
            "SecretString": "the client's own value"
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateSecret to succeed, got {status} with \
         body {created}"
    );
    let held = created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ARN in {created}"))
        .to_string();

    let template = json!({
        "Resources": {
            SECRET_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": { "Name": secret_name, "SecretString": "the stack's value" }
            }
        },
        "Outputs": { "SecretRef": { "Value": { "Ref": SECRET_LOGICAL_ID } } }
    })
    .to_string();
    create_stack(&router, case, "doomedsecretstack", &template, &[]).await;

    let (status, body) =
        wait_for_stack_status(&router, "doomedsecretstack", "ROLLBACK_COMPLETE").await;
    assert_eq!(
        status, "ROLLBACK_COMPLETE",
        "case {case:?}: expected the refused create to be rolled back, got {body}"
    );

    // The secret the CLIENT created is untouched: the stack's own value never reached it.
    let (status, read) =
        secretsmanager(&router, "GetSecretValue", &json!({ "SecretId": held })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's secret to still be served, got {status} with \
         body {read}"
    );
    assert_eq!(
        read["SecretString"].as_str(),
        Some("the client's own value"),
        "case {case:?}: expected the client's own value to be untouched, got {read}"
    );
}

/// RULE: deleting the stack deletes the secret it created in the owning service, so the ARN
/// the stack published no longer serves a value.
#[tokio::test]
async fn deleting_the_stack_deletes_the_secret_it_created() {
    let router = build_router();
    let case = "a stack whose secret is deleted with it";
    let stack_name = "deletedsecretstack";
    let template = json!({
        "Resources": {
            SECRET_LOGICAL_ID: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": format!("{stack_name}-value"),
                    "SecretString": "a-removable-value"
                }
            }
        },
        "Outputs": { "SecretRef": { "Value": { "Ref": SECRET_LOGICAL_ID } } }
    })
    .to_string();
    let stack_id = create_stack(&router, case, stack_name, &template, &[]).await;
    let (_, body) = wait_for_stack_status(&router, stack_name, "CREATE_COMPLETE").await;
    let published = crate::query_wire::outputs_of(&body)
        .get("SecretRef")
        .unwrap_or_else(|| panic!("case {case:?}: no SecretRef output in {body}"))
        .clone();

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    // A deleted stack is addressed by its ID: CloudFormation answers nothing for the NAME of
    // a stack it no longer holds.
    let (status, body) = wait_for_stack_status(&router, &stack_id, "DELETE_COMPLETE").await;
    assert_eq!(
        status, "DELETE_COMPLETE",
        "case {case:?}: expected the stack's deletion to complete, got {body}"
    );

    let (status, read) = secretsmanager(
        &router,
        "GetSecretValue",
        &json!({ "SecretId": published }),
    )
    .await;
    assert_ne!(
        status, 200,
        "case {case:?}: expected the deleted secret's ARN to serve no value, got {status} with \
         body {read}"
    );
}
