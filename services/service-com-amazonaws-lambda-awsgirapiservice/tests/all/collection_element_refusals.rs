// SPDX-License-Identifier: CC0-1.0

//! WHERE this service states a constraint one of a COLLECTION's ELEMENTS broke, on the
//! wire.
//!
//! The rule under test: a value one element of a list does not satisfy is refused
//! `400 ValidationException` whose message states ONE clause at the LIST MEMBER — no
//! element position and no `member` step — quoting the whole list that member held and
//! nesting, inside `Member must satisfy constraint: [ … ]`, every requirement an element of
//! that list has: the constraints its element shape declares, then that an element must be
//! present at all (no list this model declares is `@sparse`). Two bad elements of one list
//! are still ONE clause, because the clause is about the member and not about an element.
//!
//! A constraint broken by the LIST ITSELF (its own `@length`) is stated at the same member
//! and is NOT nested, and a constraint broken by a SCALAR member keeps its own path and
//! quotes its own value. Nothing about either is this suite's subject beyond showing that
//! the nesting does not reach them.
//!
//! The rows span what the rule generalises over: the operation that asked, whether the list
//! member sits at the top level of the input or inside a structure the input nests, the
//! constraint CLASSES the element shape declares (an enum value set; a length pair with a
//! pattern), how many of the list's elements are bad, and how many of the request's lists
//! are bad at once.
//!
//! Every refused request also persists NOTHING: a provider a refused create named is not
//! readable afterwards.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCapacityProvider` is bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider")`
/// (`POST /2025-11-30/capacity-providers`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `CreateFunction` is bound, from
/// `smithy_query(query="operation", operation="CreateFunction")`
/// (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The error code the framework answers a broken input constraint with on this `restJson1`
/// service.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The requirement every element of a list that is not `@sparse` carries beyond the
/// constraints its shape declares.
const ELEMENT_NOT_NULL: &str = "Member must not be null";

/// What an element of `ArchitecturesList` has to satisfy: `Architecture` is an enum whose
/// values are `x86_64` and `arm64`, in that declaration order
/// (`smithy_query(query="shape", shape="ArchitecturesList,Architecture")`).
fn architecture_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must satisfy enum value set: \
         [x86_64, arm64], {ELEMENT_NOT_NULL}]"
    )
}

/// What an element of either subnet list has to satisfy: `SubnetId` is
/// `@length(min: 0, max: 1024)` and `@pattern("^subnet-[0-9a-z]*$")`
/// (`smithy_query(query="shape", shape="CapacityProviderSubnetIds,SubnetIds")`). The
/// expression is quoted without the anchors a whole-value pattern wraps it in, as every
/// `@pattern` requirement of this service is.
fn subnet_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to \
         1024, Member must have length greater than or equal to 0, Member must satisfy \
         regular expression pattern: subnet-[0-9a-z]*, {ELEMENT_NOT_NULL}]"
    )
}

/// What an element of either security-group list has to satisfy: `SecurityGroupId` is
/// `@length(min: 0, max: 1024)` and `@pattern("^sg-[0-9a-zA-Z]*$")`
/// (`smithy_query(query="shape",
/// shape="CapacityProviderSecurityGroupIds,SecurityGroupIds")`).
fn security_group_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to \
         1024, Member must have length greater than or equal to 0, Member must satisfy \
         regular expression pattern: sg-[0-9a-zA-Z]*, {ELEMENT_NOT_NULL}]"
    )
}

/// What an element of `InstanceTypeSet` has to satisfy: `InstanceType` is
/// `@length(min: 1, max: 30)` and `@pattern("^[a-zA-Z0-9\\.\\*\\-]+$")`
/// (`smithy_query(query="shape", shape="InstanceTypeSet")`).
fn instance_type_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to 30, \
         Member must have length greater than or equal to 1, Member must satisfy regular \
         expression pattern: [a-zA-Z0-9\\.\\*\\-]+, {ELEMENT_NOT_NULL}]"
    )
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code the body names, or the empty string for a success.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `message` the refusal carries, or the empty string when it carries none.
    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// A `CreateCapacityProvider` body for `name` carrying both required members, which a row
/// then overrides what it is about.
fn provider_request(name: &str, patch: Value) -> Value {
    let mut request = json!({
        "CapacityProviderName": name,
        "VpcConfig": {
            "SubnetIds": ["subnet-0123456789abcdef0"],
            "SecurityGroupIds": [],
        },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
    });
    if let (Value::Object(target), Value::Object(members)) = (&mut request, &patch) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    request
}

/// One row: a request whose lists are judged, and the message it must be refused with.
struct ElementCase {
    label: &'static str,
    /// The capacity provider the row creates, for the rows that create one.
    provider: Option<&'static str>,
    uri: &'static str,
    body: Value,
    expected: String,
}

/// The rows, each a real request this service refuses.
fn cases() -> Vec<ElementCase> {
    vec![
        ElementCase {
            label: "one element outside its enum value set, in a nested structure",
            provider: Some("cp-element-arch"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-element-arch",
                json!({ "InstanceRequirements": { "Architectures": ["x86"] } }),
            ),
            expected: format!(
                "1 validation error detected: Value '[x86]' at \
                 'instanceRequirements.architectures' failed to satisfy constraint: {}",
                architecture_element_set()
            ),
        },
        ElementCase {
            label: "one element outside its pattern, in a nested structure",
            provider: Some("cp-element-subnet"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-element-subnet",
                json!({
                    "VpcConfig": {
                        "SubnetIds": ["subnet-0123456789abcdef0", "not-a-subnet"],
                        "SecurityGroupIds": [],
                    },
                }),
            ),
            expected: format!(
                "1 validation error detected: Value '[subnet-0123456789abcdef0, \
                 not-a-subnet]' at 'vpcConfig.subnetIds' failed to satisfy constraint: {}",
                subnet_element_set()
            ),
        },
        ElementCase {
            label: "TWO elements of one list outside its pattern: still one clause",
            provider: Some("cp-element-subnets-two"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-element-subnets-two",
                json!({
                    "VpcConfig": {
                        "SubnetIds": ["nope", "subnet-0123456789abcdef0", "also-nope"],
                        "SecurityGroupIds": [],
                    },
                }),
            ),
            expected: format!(
                "1 validation error detected: Value '[nope, subnet-0123456789abcdef0, \
                 also-nope]' at 'vpcConfig.subnetIds' failed to satisfy constraint: {}",
                subnet_element_set()
            ),
        },
        ElementCase {
            label: "an element shape whose length pair differs from the others'",
            provider: Some("cp-element-instance-type"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-element-instance-type",
                json!({
                    "InstanceRequirements": {
                        "AllowedInstanceTypes": ["c6g.large", "an instance type!"],
                    },
                }),
            ),
            expected: format!(
                "1 validation error detected: Value '[c6g.large, an instance type!]' at \
                 'instanceRequirements.allowedInstanceTypes' failed to satisfy constraint: {}",
                instance_type_element_set()
            ),
        },
        ElementCase {
            label: "TWO of the request's lists bad at once: one clause each",
            provider: Some("cp-element-two-lists"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-element-two-lists",
                json!({
                    "VpcConfig": {
                        "SubnetIds": ["subnet-0123456789abcdef0"],
                        "SecurityGroupIds": ["not-a-group"],
                    },
                    "InstanceRequirements": { "Architectures": ["x86"] },
                }),
            ),
            expected: format!(
                "2 validation errors detected: Value '[not-a-group]' at \
                 'vpcConfig.securityGroupIds' failed to satisfy constraint: {}; Value \
                 '[x86]' at 'instanceRequirements.architectures' failed to satisfy \
                 constraint: {}",
                security_group_element_set(),
                architecture_element_set()
            ),
        },
        ElementCase {
            label: "the list's OWN length: stated at the same member, and not nested",
            provider: Some("cp-list-length"),
            uri: CAPACITY_PROVIDERS_URI,
            body: provider_request(
                "cp-list-length",
                // `ArchitecturesList` is `@length(min: 1, max: 1)`
                // (`smithy_query(query="shape", shape="ArchitecturesList")`), so two
                // admissible elements break the LIST's constraint and not an element's.
                json!({ "InstanceRequirements": { "Architectures": ["x86_64", "arm64"] } }),
            ),
            expected: "1 validation error detected: Value '[x86_64, arm64]' at \
                       'instanceRequirements.architectures' failed to satisfy constraint: \
                       Member must have length less than or equal to 1"
                .to_string(),
        },
        ElementCase {
            label: "a SCALAR member outside its own pattern: its own path, its own value",
            provider: Some("cp-scalar-kms"),
            uri: CAPACITY_PROVIDERS_URI,
            // `KmsKeyArn` is `KMSKeyArnNonEmpty`, `@pattern("^arn:(aws[a-zA-Z-]*)?:\
            // [a-z0-9-.]+:.*$")` (`smithy_query(query="shape",
            // shape="CreateCapacityProviderRequest")`), so a value that is no ARN at all is
            // outside it — and the clause quotes that value at that member, with nothing
            // nested.
            body: provider_request("cp-scalar-kms", json!({ "KmsKeyArn": "not-an-arn" })),
            expected: "1 validation error detected: Value 'not-an-arn' at 'kmsKeyArn' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*"
                .to_string(),
        },
        ElementCase {
            label: "another operation, and a list member at the input's top level",
            provider: None,
            uri: FUNCTIONS_URI,
            body: json!({
                "FunctionName": "element-probe",
                "Role": "arn:aws:iam::000000000000:role/service-role/lambda-exec",
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": "UEsDBAoAAAAAAA==" },
                "Architectures": ["x86"],
            }),
            expected: format!(
                "1 validation error detected: Value '[x86]' at 'architectures' failed to \
                 satisfy constraint: {}",
                architecture_element_set()
            ),
        },
    ]
}

#[tokio::test]
async fn a_constraint_an_elements_value_broke_is_stated_at_its_collection() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in cases() {
        let answer = call(&router, "POST", case.uri, &case.body).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, VALIDATION_EXCEPTION),
            "case {:?}: expected 400 {VALIDATION_EXCEPTION}, got {} {}",
            case.label,
            answer.status,
            answer.text()
        );
        let reported = answer.message();
        assert_eq!(
            reported, case.expected,
            "case {:?}: expected {:?}, got {reported:?}",
            case.label, case.expected
        );

        // A refused request persists nothing, so the provider it named is not readable.
        if let Some(provider) = case.provider {
            let read = call(
                &router,
                "GET",
                &format!("{CAPACITY_PROVIDERS_URI}/{provider}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                read.status, 404,
                "case {:?}: a refused create must store no provider, got {} {}",
                case.label,
                read.status,
                read.text()
            );
        }
    }
}

#[tokio::test]
async fn a_list_whose_every_element_is_admissible_is_served() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The other side of the rule: the same members, with every element inside the
    // constraints the clauses above quote, are stored and reported back. One row per
    // element shape the table refuses a value of.
    let rows = [
        (
            "the other architecture of the value set",
            "cp-ok-arm64",
            json!({ "InstanceRequirements": { "Architectures": ["arm64"] } }),
        ),
        (
            "instance types inside their length and pattern",
            "cp-ok-instance-types",
            json!({
                "InstanceRequirements": {
                    "Architectures": ["x86_64"],
                    "AllowedInstanceTypes": ["c6g.large", "m5.*"],
                    "ExcludedInstanceTypes": ["t2.nano"],
                },
            }),
        ),
        (
            "a subnet and a security group inside their patterns",
            "cp-ok-network",
            json!({
                "VpcConfig": {
                    "SubnetIds": ["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                },
            }),
        ),
    ];

    for (case, provider, patch) in rows {
        let answer = call(
            &router,
            "POST",
            CAPACITY_PROVIDERS_URI,
            &provider_request(provider, patch),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: expected the create to be accepted, got {} {}",
            answer.status,
            answer.text()
        );
        let read = call(
            &router,
            "GET",
            &format!("{CAPACITY_PROVIDERS_URI}/{provider}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the created provider to read back, got {} {}",
            read.status,
            read.text()
        );
    }
}
