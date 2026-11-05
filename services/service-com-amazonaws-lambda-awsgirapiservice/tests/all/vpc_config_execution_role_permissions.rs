// SPDX-License-Identifier: CC0-1.0

//! What the EXECUTION ROLE of a function has to be allowed before Lambda will attach that
//! function to a VPC.
//!
//! RULE: a call that WRITES a `VpcConfig` naming a subnet or a security group —
//! `CreateFunction`, and `UpdateFunctionConfiguration` when it adds or replaces one — is
//! refused with the modelled `InvalidParameterValueException` (HTTP 400), persisting nothing,
//! when the identity-based policies in force on that function's execution role do not allow
//! every EC2 network interface call attaching a function to a VPC takes; the refusal names
//! the first such call the role may not make. "To connect a function to a VPC, Lambda creates
//! an elastic network interface for each subnet in your function's VPC configuration"
//! (`VpcConfig`) and makes those calls acting as the function's own role, so a configuration
//! the role could never stand up is a function that could never run in the VPC.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! mounted beside Lambda on ONE engine — each role is created through the real `CreateRole`
//! and `PutRolePolicy`, so what the gate reads is the document IAM persisted — and reads the
//! outcome back through `GetFunctionConfiguration`, so a configuration that was refused but
//! still stored fails here.
//!
//! The rows differ along what the rule generalises over: WHICH of the calls the role is
//! granted (all, all but one, none, one of them explicitly denied beside the grant), HOW the
//! grant names the action and the resource (exactly, through a namespace wildcard, through an
//! ARN wildcard, on another account's interfaces), WHICH call writes the configuration
//! (`CreateFunction` and `UpdateFunctionConfiguration`), and whether the configuration asks
//! for an attachment at all — one that names neither a subnet nor a group removes the
//! attachment and takes no grant. The last test pins the case where nothing is KNOWN about
//! the role, which refuses nothing.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{allow, assert_refused, create_function_as, given_role, logs_only};
use crate::event_source_mappings::{call, router_on, Answer};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The IAM action namespace the calls an attachment takes are named in.
const EC2: &str = "ec2";

/// The calls Lambda makes on the function's behalf to attach it to a VPC and to clean up
/// after it. Source: the actions AWS's own managed execution-role policy for VPC access
/// (`AWSLambdaVPCAccessExecutionRole`) grants.
const ENI_CALLS: [&str; 3] = [
    "CreateNetworkInterface",
    "DescribeNetworkInterfaces",
    "DeleteNetworkInterface",
];

/// The network interfaces of this account in the Region the functions below are configured
/// in — the resource the calls are made on.
const INTERFACES: &str = "arn:aws:ec2:us-east-1:000000000000:network-interface/*";

/// The same resource named with wildcards, as a least-privilege policy writes it.
const INTERFACES_WILDCARD: &str = "arn:aws:ec2:*:*:network-interface/*";

/// Another account's interfaces, for the row whose grant names the wrong resource.
const OTHER_ACCOUNT_INTERFACES: &str = "arn:aws:ec2:us-east-1:000000000001:network-interface/*";

/// A VPC configuration that asks for an attachment: two subnets to create an interface in and
/// the security group to apply to each.
fn attaching_config() -> Value {
    json!({
        "SubnetIds": ["subnet-0a1b2c3d4e5f60718", "subnet-0a1b2c3d4e5f60719"],
        "SecurityGroupIds": ["sg-0a1b2c3d4e5f60718"],
    })
}

/// How Lambda words the refusal of a role that may not make `call`.
fn refusal(action: &str) -> String {
    format!("The provided execution role does not have permissions to call {action} on EC2")
}

/// Lambda and IAM on ONE engine — exactly what `core/router` assembles — with the engine
/// handed back so a row can settle the clock-owed moves without sleeping.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Move every stored function's activation instant into the past, so the next call sees the
/// `Pending` -> `Active` move the clock owes. This is what waiting would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// `UpdateFunctionConfiguration` on the function `name`.
async fn update_configuration(router: &MultiServiceRouter, name: &str, patch: &Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        patch,
    )
    .await
}

/// `GetFunctionConfiguration` on the function `name`.
async fn get_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await
}

/// The `VpcConfig` a configuration body reports, or `Value::Null` when it reports none.
fn reported_config(answer: &Answer) -> Value {
    answer
        .json()
        .get("VpcConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One row: the statements of the role's one inline policy, the VPC configuration the call
/// writes, and the sentence it must be refused with (`None` when it must be served).
struct Row {
    case: &'static str,
    statements: Value,
    vpc_config: Value,
    refusal: Option<String>,
}

/// The catalog: which of the calls the role is granted, how the grant names them, and whether
/// the configuration asks for an attachment at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: a role carrying only the log writes.
            case: "only the log writes granted",
            statements: json!([logs_only()]),
            vpc_config: attaching_config(),
            refusal: Some(refusal(ENI_CALLS[0])),
        },
        Row {
            // The positive control: exactly the three calls, on this account's interfaces.
            case: "every call granted on this account's interfaces",
            statements: json!([logs_only(), allow(EC2, &ENI_CALLS, INTERFACES)]),
            vpc_config: attaching_config(),
            refusal: None,
        },
        Row {
            case: "granted through a namespace wildcard",
            statements: json!([allow(EC2, &["*"], "*")]),
            vpc_config: attaching_config(),
            refusal: None,
        },
        Row {
            case: "granted through an ARN wildcard",
            statements: json!([allow(EC2, &ENI_CALLS, INTERFACES_WILDCARD)]),
            vpc_config: attaching_config(),
            refusal: None,
        },
        Row {
            case: "granted on another account's interfaces",
            statements: json!([allow(EC2, &ENI_CALLS, OTHER_ACCOUNT_INTERFACES)]),
            vpc_config: attaching_config(),
            refusal: Some(refusal(ENI_CALLS[0])),
        },
        Row {
            // The create is granted, so the refusal names the first call that is not.
            case: "one call short of the set",
            statements: json!([allow(
                EC2,
                &[ENI_CALLS[0], ENI_CALLS[1]],
                INTERFACES
            )]),
            vpc_config: attaching_config(),
            refusal: Some(refusal(ENI_CALLS[2])),
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over the grant beside it.
            case: "every call granted and one denied",
            statements: json!([
                allow(EC2, &ENI_CALLS, INTERFACES),
                {
                    "Effect": "Deny",
                    "Action": format!("{EC2}:{}", ENI_CALLS[1]),
                    "Resource": INTERFACES,
                },
            ]),
            vpc_config: attaching_config(),
            refusal: Some(refusal(ENI_CALLS[1])),
        },
        Row {
            // "To remove the function's VPC configuration, pass an empty list": such a
            // configuration stands up no interface, so it takes no grant.
            case: "a configuration that removes the attachment",
            statements: json!([logs_only()]),
            vpc_config: json!({ "SubnetIds": [], "SecurityGroupIds": [] }),
            refusal: None,
        },
        Row {
            case: "a configuration naming only a security group",
            statements: json!([logs_only()]),
            vpc_config: json!({ "SecurityGroupIds": ["sg-0a1b2c3d4e5f60718"] }),
            refusal: Some(refusal(ENI_CALLS[0])),
        },
    ]
}

#[tokio::test]
async fn an_update_attaches_a_function_only_where_its_role_may_create_the_interfaces() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator();
        let role = given_role(
            &router,
            case,
            &format!("vpc-attacher-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let function = format!("vpc-attach-probe-{index}");
        create_function_as(&router, case, &function, &role).await;
        // A function is not modifiable until it has finished being created, so the row starts
        // from the settled record a caller that waited would see.
        make_activation_due(&engine);

        let answer =
            update_configuration(&router, &function, &json!({ "VpcConfig": row.vpc_config })).await;

        if let Some(expected) = row.refusal.as_deref() {
            assert_refused(case, &answer, expected);
            // The refusal is instead of the configuration, not beside it: the function is
            // left attached to nothing.
            let reading = get_configuration(&router, &function).await;
            assert_eq!(
                (reading.status, reported_config(&reading)),
                (200, Value::Null),
                "case {case:?}: a refused update must leave no VpcConfig behind, got {} {}",
                reading.status,
                reading.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            answer.text()
        );
        // Round trip: what the update answered is what the store holds. A configuration that
        // removes the attachment is reported as none at all.
        let reading = get_configuration(&router, &function).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration, got {} {}",
            reading.status,
            reading.text()
        );
        let stored = reported_config(&reading);
        for member in ["SubnetIds", "SecurityGroupIds"] {
            let asked = row
                .vpc_config
                .get(member)
                .cloned()
                .unwrap_or(json!([]));
            let held = stored.get(member).cloned().unwrap_or(json!([]));
            assert_eq!(
                held, asked,
                "case {case:?}: expected {member} to read back as {asked}, got {held} in {}",
                reading.text()
            );
        }
    }
}

#[tokio::test]
async fn a_create_attaches_a_function_only_where_its_role_may_create_the_interfaces() {
    // (case, the role's statements, the refusal the create must answer)
    let rows: [(&str, Value, Option<String>); 3] = [
        (
            "only the log writes granted",
            json!([logs_only()]),
            Some(refusal(ENI_CALLS[0])),
        ),
        (
            "every call granted on this account's interfaces",
            json!([allow(EC2, &ENI_CALLS, INTERFACES)]),
            None,
        ),
        (
            "the delete missing from the grant",
            json!([allow(EC2, &[ENI_CALLS[0], ENI_CALLS[1]], INTERFACES)]),
            Some(refusal(ENI_CALLS[2])),
        ),
    ];

    for (index, (case, statements, refused)) in rows.into_iter().enumerate() {
        let (router, _engine) = emulator();
        let role = given_role(
            &router,
            case,
            &format!("vpc-creator-{index}"),
            Some(statements),
        )
        .await;
        let function = format!("vpc-create-probe-{index}");
        let answer = call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &json!({
                "FunctionName": function,
                "Role": role,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": "" },
                "VpcConfig": attaching_config(),
            }),
        )
        .await;

        match refused {
            Some(expected) => {
                assert_refused(case, &answer, &expected);
                // Nothing was stored: the function the refused create named does not exist.
                let reading = get_configuration(&router, &function).await;
                assert_eq!(
                    reading.status, 404,
                    "case {case:?}: a refused create must store no function, got {} {}",
                    reading.status,
                    reading.text()
                );
            }
            None => {
                assert_eq!(
                    answer.status,
                    201,
                    "case {case:?}: expected HTTP 201, got {} {}",
                    answer.status,
                    answer.text()
                );
                let reading = get_configuration(&router, &function).await;
                assert_eq!(
                    reported_config(&reading).get("SubnetIds").cloned(),
                    attaching_config().get("SubnetIds").cloned(),
                    "case {case:?}: the created function must hold the subnets it asked for, \
                     got {}",
                    reading.text()
                );
            }
        }
    }
}

#[tokio::test]
async fn a_role_nothing_is_known_about_refuses_nothing() {
    // A role ARN IAM holds no record of: an execution role is normally provisioned outside
    // the client that creates the function, so an absent record is no evidence about what the
    // role may do, and the attachment is served.
    let case = "a role this account's IAM holds no record of";
    let (router, engine) = emulator();
    let role = "arn:aws:iam::000000000000:role/service-role/provisioned-elsewhere";
    create_function_as(&router, case, "vpc-unknown-role", role).await;
    make_activation_due(&engine);

    let answer = update_configuration(
        &router,
        "vpc-unknown-role",
        &json!({ "VpcConfig": attaching_config() }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {}",
        answer.status,
        answer.text()
    );
    let reading = get_configuration(&router, "vpc-unknown-role").await;
    assert_eq!(
        reported_config(&reading).get("SubnetIds").cloned(),
        attaching_config().get("SubnetIds").cloned(),
        "case {case:?}: the attachment must read back, got {}",
        reading.text()
    );
}
