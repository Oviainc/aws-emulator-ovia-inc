// SPDX-License-Identifier: CC0-1.0

//! `CreateEndpoint`: the service access role an endpoint is created with is resolved
//! against the account's IAM records BEFORE the endpoint is persisted.
//!
//! The rule under test: a member of a create whose value names the IAM role the service
//! will act as is resolved against the account's roles at create time — the check is not
//! deferred to a later call — and a value IAM answers for as a role this account does not
//! hold, or one whose trust policy does not admit the DMS service principal, is refused
//! with the modelled `AccessDeniedFault` (HTTP 400) with nothing persisted. A value IAM
//! was not asked about, because it names no role of THIS account, grounds no refusal:
//! another account's IAM is not this emulator's to answer for.
//!
//! The rows span what the rule generalises over: WHICH member carries the role (the
//! endpoint's own `ServiceAccessRoleArn` and the per-engine settings copies of it, over
//! the engines that take one), and WHAT the value names (a role of this account trusted by
//! DMS, the same role under a path, a role that exists but trusts another service, a
//! well-formed ARN naming a role the account does not hold, a role of another account, and
//! a value that is not a role ARN at all).
//!
//! IAM is mounted beside this service on ONE engine and the roles are created through
//! IAM's own operations, so a role exists only because IAM answered for it, and the
//! resolution under test runs over the same in-process callback as in production.

use serde_json::{json, Value};

use crate::migration_task_slice::{error_code, text, Emulator};

/// The error code DMS answers a role it cannot use with.
const ACCESS_DENIED: &str = "AccessDeniedFault";

/// The error code a filtered describe answers when it narrows to nothing.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundFault";

/// The principal a role DMS acts as has to trust.
const DMS_PRINCIPAL: &str = "dms.amazonaws.com";

/// A service principal that is NOT DMS, for the role that exists and trusts someone else.
const OTHER_PRINCIPAL: &str = "ec2.amazonaws.com";

/// A trust policy admitting `principal` to assume the role.
fn trust(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create `role_name` under `path` with `trust_document`, through IAM itself, and answer
/// the ARN IAM minted for it.
async fn create_role(
    iam: &::aws_sdk_iam::Client,
    path: &str,
    role_name: &str,
    trust_document: &str,
) -> String {
    iam.create_role()
        .role_name(role_name)
        .path(path)
        .assume_role_policy_document(trust_document)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: IAM must create the role {role_name}: {error:?}"))
        .role()
        .map(|role| role.arn().to_string())
        .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {role_name}"))
}

/// The roles the rows are built from, as IAM answered for them.
struct Roles {
    /// A role of this account whose trust policy admits DMS.
    usable: String,
    /// The same, under a path — the name IAM holds it under is the last segment.
    usable_under_path: String,
    /// A role of this account that exists but trusts another service.
    untrusted: String,
}

/// Where in the request a row puts its role ARN, and what the engine settings around it
/// have to carry for the request to be well formed.
///
/// The members are the ones this service's model declares a `ServiceAccessRoleArn` on, as
/// `smithy_query(query="operation", operation="CreateEndpoint")` lists them: the endpoint's
/// own member and the per-engine settings structures'.
struct Member {
    /// What the member is called in a case name.
    name: &'static str,
    /// The `EngineName` the request has to carry for that settings structure.
    engine_name: &'static str,
    /// The request body carrying `role_arn` in this member.
    body: fn(&str) -> Value,
}

/// Every member the rule covers.
fn members() -> Vec<Member> {
    vec![
        Member {
            name: "the endpoint's own ServiceAccessRoleArn",
            engine_name: "s3",
            body: |role_arn| json!({ "ServiceAccessRoleArn": role_arn }),
        },
        Member {
            name: "S3Settings.ServiceAccessRoleArn",
            engine_name: "s3",
            body: |role_arn| {
                json!({
                    "S3Settings": {
                        "BucketName": "landing-zone",
                        "BucketFolder": "out",
                        "ServiceAccessRoleArn": role_arn,
                    },
                })
            },
        },
        Member {
            name: "DynamoDbSettings.ServiceAccessRoleArn",
            engine_name: "dynamodb",
            body: |role_arn| json!({ "DynamoDbSettings": { "ServiceAccessRoleArn": role_arn } }),
        },
        Member {
            name: "KinesisSettings.ServiceAccessRoleArn",
            engine_name: "kinesis",
            body: |role_arn| {
                json!({
                    "KinesisSettings": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "ServiceAccessRoleArn": role_arn,
                    },
                })
            },
        },
        Member {
            name: "ElasticsearchSettings.ServiceAccessRoleArn",
            engine_name: "opensearch",
            body: |role_arn| {
                json!({
                    "ElasticsearchSettings": {
                        "EndpointUri": "search.example.internal",
                        "ServiceAccessRoleArn": role_arn,
                    },
                })
            },
        },
        Member {
            name: "NeptuneSettings.ServiceAccessRoleArn",
            engine_name: "neptune",
            body: |role_arn| {
                json!({
                    "NeptuneSettings": {
                        "S3BucketName": "neptune-staging",
                        "S3BucketFolder": "bulk",
                        "ServiceAccessRoleArn": role_arn,
                    },
                })
            },
        },
        Member {
            name: "DmsTransferSettings.ServiceAccessRoleArn",
            engine_name: "dms-transfer",
            body: |role_arn| {
                json!({
                    "DmsTransferSettings": {
                        "BucketName": "transfer-zone",
                        "ServiceAccessRoleArn": role_arn,
                    },
                })
            },
        },
    ]
}

/// One role value the rule is asserted over.
struct RoleValue {
    /// What this row varies.
    case: &'static str,
    /// The value the member carries.
    role_arn: String,
    /// Whether the create must be accepted.
    accepted: bool,
}

/// Every value, each asserted through every member above.
fn values(roles: &Roles) -> Vec<RoleValue> {
    vec![
        RoleValue {
            case: "a role of this account that trusts DMS",
            role_arn: roles.usable.clone(),
            accepted: true,
        },
        RoleValue {
            case: "the same role under a path",
            role_arn: roles.usable_under_path.clone(),
            accepted: true,
        },
        RoleValue {
            // IAM holds no role under that name, which is what the recorded refusal is
            // about: a well-formed, account-correct ARN naming a role that is not there.
            case: "a well-formed ARN naming a role the account does not hold",
            role_arn: format!("{}-absent", roles.usable),
            accepted: false,
        },
        RoleValue {
            case: "a role that exists but trusts another service",
            role_arn: roles.untrusted.clone(),
            accepted: false,
        },
        RoleValue {
            // This account's IAM cannot answer for another account's roles, so nothing
            // grounds a refusal.
            case: "a role of another account",
            role_arn: "arn:aws:iam::123456789012:role/dms-cross-account".to_string(),
            accepted: true,
        },
        RoleValue {
            case: "a value that names no IAM role at all",
            role_arn: "dms-access".to_string(),
            accepted: true,
        },
    ]
}

/// Whether the account holds an endpoint under `identifier`, read back through the
/// filtered describe.
async fn endpoint_is_held(emulator: &Emulator, identifier: &str) -> bool {
    let (status, response) = emulator
        .call(
            "DescribeEndpoints",
            &json!({ "Filters": [{ "Name": "endpoint-id", "Values": [identifier] }] }),
        )
        .await;
    if status == 200 {
        return response
            .pointer("/Endpoints")
            .and_then(Value::as_array)
            .is_some_and(|endpoints| !endpoints.is_empty());
    }
    assert_eq!(
        error_code(&response),
        RESOURCE_NOT_FOUND,
        "a describe narrowed to nothing must raise {RESOURCE_NOT_FOUND}, got \
         {status} {response}"
    );
    false
}

#[tokio::test]
async fn an_endpoints_service_access_role_is_resolved_before_the_endpoint_is_persisted() {
    let (emulator, _ec2, iam) = Emulator::with_network();
    let roles = Roles {
        usable: create_role(&iam, "/", "dms-endpoint-access", &trust(DMS_PRINCIPAL)).await,
        usable_under_path: create_role(
            &iam,
            "/service-role/",
            "dms-endpoint-access-pathed",
            &trust(DMS_PRINCIPAL),
        )
        .await,
        untrusted: create_role(&iam, "/", "dms-endpoint-denied", &trust(OTHER_PRINCIPAL)).await,
    };

    for (member_index, member) in members().into_iter().enumerate() {
        for (value_index, value) in values(&roles).into_iter().enumerate() {
            let case = format!("{} with {}", member.name, value.case);
            let identifier = format!("ep-{member_index}-{value_index}");
            let mut request = json!({
                "EndpointIdentifier": identifier,
                "EndpointType": "target",
                "EngineName": member.engine_name,
            });
            for (key, carried) in (member.body)(&value.role_arn)
                .as_object()
                .expect("a member body is an object")
            {
                request[key] = carried.clone();
            }

            let (status, response) = emulator.call("CreateEndpoint", &request).await;
            if value.accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be accepted, got \
                     {status} {response}"
                );
                assert!(
                    endpoint_is_held(&emulator, &identifier).await,
                    "case {case:?}: an accepted create must persist the endpoint"
                );
            } else {
                let code = error_code(&response);
                assert_eq!(
                    (status, code.as_str()),
                    (400, ACCESS_DENIED),
                    "case {case:?}: expected the create to be refused with \
                     {ACCESS_DENIED}, got {status} {response}"
                );
                // The refusal names the role ARN the REQUEST carried.
                let message = text(&response, "/message");
                assert!(
                    message.contains(&value.role_arn),
                    "case {case:?}: expected the refusal to name {}, got {message:?}",
                    value.role_arn
                );
                assert!(
                    !endpoint_is_held(&emulator, &identifier).await,
                    "case {case:?}: a refused create must persist nothing"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_endpoint_that_names_no_role_is_created_whatever_iam_holds() {
    let (emulator, _ec2, iam) = Emulator::with_network();
    create_role(&iam, "/", "dms-unused", &trust(DMS_PRINCIPAL)).await;

    // Nothing to resolve: the rule only speaks about a role a request NAMES, so an
    // endpoint that names none is unaffected by it.
    let created = emulator
        .ok(
            "no role named",
            "CreateEndpoint",
            &json!({
                "EndpointIdentifier": "ep-no-role",
                "EndpointType": "source",
                "EngineName": "mysql",
                "ServerName": "db.example.internal",
                "Port": 3306,
            }),
        )
        .await;
    assert!(
        created.pointer("/Endpoint/ServiceAccessRoleArn").is_none(),
        "expected no ServiceAccessRoleArn on an endpoint that named none, got {created}"
    );
    assert!(
        endpoint_is_held(&emulator, "ep-no-role").await,
        "expected the endpoint to be persisted"
    );
}
