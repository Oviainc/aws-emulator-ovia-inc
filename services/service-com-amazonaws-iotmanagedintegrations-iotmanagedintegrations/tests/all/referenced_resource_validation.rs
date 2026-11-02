// SPDX-License-Identifier: CC0-1.0

//! The resolution of the resources a create REFERS TO, end to end over the wire.
//!
//! RULE: a create that names another resource — by identifier or by ARN — resolves that
//! reference BEFORE it stores anything, and answers the modelled `ValidationException`
//! (HTTP 400), persisting nothing, when the reference does not resolve:
//!
//! * `CreateAccountAssociation`'s `ConnectorDestinationId`, against this service's OWN
//!   connector-destination store in the region the request was signed for — an association
//!   is created "via the destination id", so it points at a destination the account has;
//! * `CreateCloudConnector`'s `EndpointConfig`, against the emulator's own Lambda — a
//!   `LAMBDA` endpoint INVOKES the function its `arn` names;
//! * `CreateDestination`'s and `UpdateDestination`'s `RoleArn`, against the account's IAM
//!   records — the delivery is performed AS that role.
//!
//! A reference nothing can be shown about is NOT refused. Lambda and IAM are asked over the
//! emulator's own in-process callback, so both are mounted beside this service here; that is
//! why every other suite in this crate, which mounts this service alone, still creates
//! connectors and destinations whose function and role no record backs.
//!
//! Each table's rows differ along what the rule generalises over: the FORM of the reference
//! (a minted id, a truncation or an extension of one, an id of another region, one a delete
//! removed; a bare function name, a function ARN; an IAM role ARN of this account, of
//! another account, of another entity type, a value that is no ARN at all) and, for a role,
//! along what IAM ANSWERS about it (held with a trust document that admits this service,
//! held with one that does not, held with none at all, not held).
//!
//! Every call goes through the real restJson1 request path (`MultiServiceRouter::dispatch`),
//! so the statuses, bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    DEFAULT_PATH,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::DELIVERY_DESTINATION_TYPE_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// Another region, for the rows about a reference that resolves only elsewhere.
const OTHER_REGION: &str = "eu-central-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The wire error code the model's `ValidationException` (`@httpError` 400) renders under.
const VALIDATION: &str = "ValidationException";

/// The wire error code the model's `ResourceNotFoundException` renders under.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The routes under test (`smithy_query(query="digest")`: POST `/account-associations`,
/// POST `/cloud-connectors`, POST `/connector-destinations`, POST `/destinations`).
const ASSOCIATIONS: &str = "/account-associations";
const CONNECTORS: &str = "/cloud-connectors";
const CONNECTOR_DESTINATIONS: &str = "/connector-destinations";
const DESTINATIONS: &str = "/destinations";

/// Where Lambda binds `CreateFunction`.
const LAMBDA_FUNCTIONS: &str = "/2015-03-31/functions";

/// The whole `Message` each refusal carries — the service's FIXED text for that error
/// class, written out here rather than read from the crate's own constants so this suite
/// pins the wording a client reads.
const DESTINATION_ABSENT: &str = "Connector Destination in the request does not exist.";
const LAMBDA_ABSENT: &str = "Connector endpoint Lambda ARN does not exist";
const ROLE_UNUSABLE: &str = "Role ARN is invalid or cannot be assumed. Please check the role \
                             configuration and permissions.";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message").unwrap_or_default()
    }

    /// The `arn` of the endpoint's Lambda configuration, as `GetCloudConnector` reports it.
    fn endpoint_lambda_arn(&self) -> Option<&str> {
        self.member("EndpointConfig")?
            .get("lambda")?
            .get("arn")?
            .as_str()
    }
}

/// An emulator with managed integrations, Lambda AND IAM mounted on ONE shared engine —
/// the assembly `core/router` performs, with three services mounted — plus that engine, so
/// a case can seed the IAM records this service asks about.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Send one request to `signing_name`'s endpoint in `region` and reduce the answer. The
/// `Authorization` credential scope is what routes the request to a service and names its
/// region.
async fn call_signed(
    router: &MultiServiceRouter,
    signing_name: &str,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://{signing_name}.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// One managed-integrations request in `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(
        router,
        "iotmanagedintegrations",
        region,
        method,
        path_and_query,
        body,
    )
    .await
}

/// Create the Lambda function `name` through Lambda's OWN `CreateFunction`, so the function
/// a connector endpoint resolves is one a client really created.
///
/// Lambda resolves the function's execution role against the same IAM records, so the role
/// is seeded first — a function has to be creatable before a connector can name it.
async fn lambda_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    let execution_role = seed_role(
        engine,
        "/service-role/",
        "lambda-exec",
        Some(trust("lambda.amazonaws.com")),
    );
    // A deployment package is a blob, which restJson1 carries base64-encoded.
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call_signed(
        router,
        "lambda",
        REGION,
        "POST",
        LAMBDA_FUNCTIONS,
        Some(json!({
            "FunctionName": name,
            "Role": execution_role,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding the Lambda function {name:?}: expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
}

/// Seed `role_name` into the shared engine's IAM store carrying `trust` as its
/// `AssumeRolePolicyDocument` — the record `CreateRole` writes — and answer the ARN IAM
/// holds it under.
fn seed_role(
    engine: &Arc<engine::InMemoryEngine>,
    path: &str,
    role_name: &str,
    trust: Option<String>,
) -> String {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let arn = role_arn(path, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: path.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: trust,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// A trust document with one statement: `principal` is allowed to assume the role.
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

/// The service principal managed integrations assumes a delivery role as, derived the way
/// AWS names one: the service's own `arnNamespace`
/// (`smithy_query(query="service_info")` -> `iotmanagedintegrations`) plus
/// `.amazonaws.com`.
const SERVICE_PRINCIPAL: &str = "iotmanagedintegrations.amazonaws.com";

/// Create a C2C connector in `region` whose endpoint names `lambda_arn`, answering the raw
/// reply so a case can assert either the acceptance or the refusal.
async fn create_connector(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    lambda_arn: &str,
) -> Reply {
    call_in(
        router,
        region,
        "POST",
        CONNECTORS,
        Some(json!({
            "Name": name,
            "EndpointConfig": { "lambda": { "arn": lambda_arn } },
        })),
    )
    .await
}

/// Create a connector destination in `region` and answer the `ConnectorDestinationId` it
/// minted: the resource an account association is created against. The connector comes
/// first, since a destination names one (`CloudConnectorId` is `@required`).
async fn connector_destination(router: &MultiServiceRouter, region: &str, handler: &str) -> String {
    let connector = create_connector(router, region, "partner cloud connector", handler).await;
    assert_eq!(
        connector.status, 201,
        "seeding a cloud connector in {region:?}: expected HTTP 201, got {} with body {:?}",
        connector.status, connector.body
    );
    let connector_id = connector.string("Id").unwrap_or_default().to_string();
    let destination = call_in(
        router,
        region,
        "POST",
        CONNECTOR_DESTINATIONS,
        Some(json!({
            "CloudConnectorId": connector_id,
            "AuthConfig": {},
            // The service requires the ARN of the secret a destination keeps its
            // credentials in. Inside the `SecretsManagerArn` `@pattern`
            // (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
            // and the `SecretsManagerVersionId` `@length` (32..64).
            "SecretsManager": {
                "arn": format!("arn:aws:secretsmanager:{region}:{ACCOUNT}:secret:destination-A1b2C3"),
                "versionId": "a".repeat(32),
            },
        })),
    )
    .await;
    assert_eq!(
        destination.status, 201,
        "seeding a connector destination in {region:?}: expected HTTP 201, got {} with \
         body {:?}",
        destination.status, destination.body
    );
    destination.string("Id").unwrap_or_default().to_string()
}

/// A `CreateDestination` body delivering into `region` as `role_arn`.
fn destination_body(region: &str, name: &str, role_arn: &str) -> Value {
    json!({
        "Name": name,
        "DeliveryDestinationArn": format!("arn:aws:kinesis:{region}:{ACCOUNT}:stream/events"),
        "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
        "RoleArn": role_arn,
    })
}

/// Assert one answer is the modelled refusal `status`/`code` carrying exactly `message`.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected the reason {message:?}, got body {:?}",
        reply.body
    );
}

/// RULE: `CreateAccountAssociation` resolves its `ConnectorDestinationId` against the
/// connector destinations the signing region holds, by equality on the minted id, and
/// refuses — storing nothing — anything that does not resolve.
#[tokio::test]
async fn an_account_association_resolves_its_connector_destination() {
    let (router, engine) = emulator();
    lambda_function(&router, &engine, "connector-handler").await;

    let live = connector_destination(&router, REGION, "connector-handler").await;
    let elsewhere = connector_destination(&router, OTHER_REGION, "connector-handler").await;
    let removed = connector_destination(&router, REGION, "connector-handler").await;
    let deleted = call_in(
        &router,
        REGION,
        "DELETE",
        &format!("{CONNECTOR_DESTINATIONS}/{removed}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "deleting a connector destination: expected HTTP 200, got {} with body {:?}",
        deleted.status, deleted.body
    );

    // The destination the region holds is the one an association is created against.
    let created = call_in(
        &router,
        REGION,
        "POST",
        ASSOCIATIONS,
        Some(json!({ "ConnectorDestinationId": live, "Name": "Partner account" })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"a destination the region holds\": expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
    let association = created
        .string("AccountAssociationId")
        .expect("a create answers with the AccountAssociationId it minted")
        .to_string();

    // What the create wrote is what a read reports: the association points at that
    // destination.
    let read = call_in(
        &router,
        REGION,
        "GET",
        &format!("{ASSOCIATIONS}/{association}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "reading the association back: expected HTTP 200, got {} with body {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.string("ConnectorDestinationId"),
        Some(live.as_str()),
        "reading the association back: expected the destination it was created against, \
         got {:?}",
        read.body
    );

    // The rows differ along the form of the reference: ids nothing minted (at both
    // modelled `@length` bounds of `ConnectorDestinationId`), a truncation and an
    // extension of a live id, the id of a destination of ANOTHER region, and the id of one
    // a delete removed.
    let rows: Vec<(String, String)> = vec![
        ("an id nothing minted".to_string(), "a".to_string()),
        (
            "an id nothing minted, at the modelled maximum length".to_string(),
            "d".repeat(64),
        ),
        (
            "a truncation of a live id".to_string(),
            live[..12].to_string(),
        ),
        (
            "a live id with a character appended".to_string(),
            format!("{live}0"),
        ),
        (
            "the id of a destination of another region".to_string(),
            elsewhere.clone(),
        ),
        (
            "the id of a destination a delete removed".to_string(),
            removed.clone(),
        ),
    ];

    for (case, reference) in rows {
        let refused = call_in(
            &router,
            REGION,
            "POST",
            ASSOCIATIONS,
            Some(json!({ "ConnectorDestinationId": reference })),
        )
        .await;
        assert_refused(&case, &refused, 400, VALIDATION, DESTINATION_ABSENT);
        assert!(
            refused.member("AccountAssociationId").is_none(),
            "case {case:?}: a refused create mints no identifier, got body {:?}",
            refused.body
        );
    }

    // Nothing was stored for any refused row: the region still holds exactly the one
    // association the accepted create wrote.
    let listed = call_in(&router, REGION, "GET", ASSOCIATIONS, None).await;
    let ids: Vec<&str> = listed
        .member("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("AccountAssociationId").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![association.as_str()],
        "a refused create stores nothing, so the store holds only the accepted record, \
         got body {:?}",
        listed.body
    );
}

/// RULE: `CreateCloudConnector` resolves the Lambda function its `EndpointConfig` names
/// against the emulator's own Lambda — in every form the modelled `LambdaArn` admits — and
/// refuses a create naming one Lambda does not hold.
#[tokio::test]
async fn a_cloud_connector_endpoint_resolves_its_lambda_function() {
    let (router, engine) = emulator();
    lambda_function(&router, &engine, "connector-handler").await;
    lambda_function(&router, &engine, "second-handler").await;

    // The accepted rows differ along the FORM the function is named by — the two the
    // `LambdaArn` pattern admits, a bare name and a full function ARN — and along which of
    // the account's functions they name, so nothing is keyed to one name.
    let accepted: Vec<(&str, String)> = vec![
        ("a bare function name", "connector-handler".to_string()),
        (
            "the full ARN of that function",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:connector-handler"),
        ),
        ("another function of the account", "second-handler".to_string()),
        (
            "the full ARN of that other function",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:second-handler"),
        ),
    ];

    for (case, lambda_arn) in accepted {
        let created = create_connector(&router, REGION, case, &lambda_arn).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {:?}",
            created.status, created.body
        );
        let id = created
            .string("Id")
            .expect("a create answers with the CloudConnectorId it minted")
            .to_string();

        // The endpoint the create was given is the endpoint the stored connector reports.
        let read = call_in(&router, REGION, "GET", &format!("{CONNECTORS}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetCloudConnector, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.endpoint_lambda_arn(),
            Some(lambda_arn.as_str()),
            "case {case:?}: expected the stored endpoint to name {lambda_arn:?}, got {:?}",
            read.body
        );
    }

    // The refused rows differ the same way, over functions this emulator's Lambda holds
    // none of: a bare name and a full ARN nothing created, a name differing only in case
    // from one it holds, and a name extending one it holds.
    let refused: Vec<(&str, String)> = vec![
        ("a bare name nothing created", "absent-handler".to_string()),
        (
            "the full ARN of a function nothing created",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent-handler"),
        ),
        (
            "a name differing only in case from one the account holds",
            "Connector-Handler".to_string(),
        ),
        (
            "a name extending one the account holds",
            "connector-handler-2".to_string(),
        ),
    ];

    for (case, lambda_arn) in refused {
        let reply = create_connector(&router, REGION, case, &lambda_arn).await;
        assert_refused(case, &reply, 400, VALIDATION, LAMBDA_ABSENT);
        assert!(
            reply.member("Id").is_none(),
            "case {case:?}: a refused create mints no identifier, got body {:?}",
            reply.body
        );
    }
}

/// RULE: `CreateDestination` resolves the `RoleArn` the delivery is performed as — the
/// value has to name an IAM role of THIS account, and IAM has to answer that this service
/// can assume it — and refuses, storing nothing, when it does not.
#[tokio::test]
async fn a_destination_resolves_the_role_it_delivers_as() {
    let (router, engine) = emulator();

    let admitted = seed_role(
        &engine,
        DEFAULT_PATH,
        "delivery",
        Some(trust(SERVICE_PRINCIPAL)),
    );
    let under_a_path = seed_role(
        &engine,
        "/service-role/",
        "scoped-delivery",
        Some(trust(SERVICE_PRINCIPAL)),
    );
    let another_service = seed_role(
        &engine,
        DEFAULT_PATH,
        "kinesis-only",
        Some(trust("kinesis.amazonaws.com")),
    );
    let untrusting = seed_role(&engine, DEFAULT_PATH, "no-trust", None);

    // The accepted rows differ along the PATH IAM holds the role under, which is part of
    // the ARN but not of the name IAM resolves it by.
    for (case, role) in [
        ("a role under the default path", &admitted),
        ("a role under a path", &under_a_path),
    ] {
        let name = format!("destination.for.{}", case.replace(' ', "-"));
        let created = call_in(
            &router,
            REGION,
            "POST",
            DESTINATIONS,
            Some(destination_body(REGION, &name, role)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {:?}",
            created.status, created.body
        );

        // What the create was given is what the stored record reports.
        let read = call_in(
            &router,
            REGION,
            "GET",
            &format!("{DESTINATIONS}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.string("RoleArn"),
            Some(role.as_str()),
            "case {case:?}: expected the stored RoleArn {role:?}, got body {:?}",
            read.body
        );
    }

    // The refused rows differ along what IAM answers about the role (held with a trust
    // document that admits another service, held with none at all, not held) and along the
    // form of the value (another account's role, another IAM entity type, an STS session,
    // text that is no ARN at all, nothing).
    let rows: Vec<(&str, String)> = vec![
        (
            "a role this account does not hold",
            format!("arn:aws:iam::{ACCOUNT}:role/absent"),
        ),
        (
            "a role whose trust document admits another service",
            another_service.clone(),
        ),
        ("a role carrying no trust document", untrusting.clone()),
        (
            "a role of another account",
            "arn:aws:iam::123456789012:role/delivery".to_string(),
        ),
        (
            "an IAM user of this account",
            format!("arn:aws:iam::{ACCOUNT}:user/delivery"),
        ),
        (
            "an instance profile of this account",
            format!("arn:aws:iam::{ACCOUNT}:instance-profile/delivery"),
        ),
        (
            "an assumed-role session of this account",
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/delivery/session-1"),
        ),
        ("a bare role name", "delivery".to_string()),
        ("a value that is no ARN at all", "not an arn".to_string()),
        ("the empty string", String::new()),
    ];

    for (case, role) in rows {
        let name = "refused.destination";
        let reply = call_in(
            &router,
            REGION,
            "POST",
            DESTINATIONS,
            Some(destination_body(REGION, name, &role)),
        )
        .await;
        assert_refused(case, &reply, 400, VALIDATION, ROLE_UNUSABLE);

        // A refused create stores nothing: the name it named holds no destination.
        let read = call_in(
            &router,
            REGION,
            "GET",
            &format!("{DESTINATIONS}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store no destination, but the read \
             answered {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some(NOT_FOUND),
            "case {case:?}: expected the modelled {NOT_FOUND} code from the read, got {:?}",
            read.error_type
        );
    }
}

/// RULE: `UpdateDestination` resolves a replacement `RoleArn` the same way, so a role this
/// service cannot deliver as is never written over one it can; an update that names no
/// role leaves the stored one alone.
#[tokio::test]
async fn an_update_resolves_the_role_it_would_deliver_as() {
    let (router, engine) = emulator();
    let first = seed_role(
        &engine,
        DEFAULT_PATH,
        "delivery",
        Some(trust(SERVICE_PRINCIPAL)),
    );
    let second = seed_role(
        &engine,
        DEFAULT_PATH,
        "other-delivery",
        Some(trust(SERVICE_PRINCIPAL)),
    );

    let name = "updated.destination";
    let created = call_in(
        &router,
        REGION,
        "POST",
        DESTINATIONS,
        Some(destination_body(REGION, name, &first)),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "seeding the destination: expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );

    // The rows differ the same way the create's do; each one must leave the stored role as
    // it was.
    let refused: Vec<(&str, String)> = vec![
        (
            "a role this account does not hold",
            format!("arn:aws:iam::{ACCOUNT}:role/absent"),
        ),
        (
            "a role of another account",
            "arn:aws:iam::123456789012:role/delivery".to_string(),
        ),
        ("a value that is no ARN at all", "delivery".to_string()),
    ];
    for (case, role) in refused {
        let reply = call_in(
            &router,
            REGION,
            "PUT",
            &format!("{DESTINATIONS}/{name}"),
            Some(json!({ "RoleArn": role, "Description": "re-pointed" })),
        )
        .await;
        assert_refused(case, &reply, 400, VALIDATION, ROLE_UNUSABLE);

        let read = call_in(
            &router,
            REGION,
            "GET",
            &format!("{DESTINATIONS}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.string("RoleArn"),
            Some(first.as_str()),
            "case {case:?}: a refused update must leave the stored RoleArn alone, got \
             body {:?}",
            read.body
        );
        assert!(
            read.string("Description").is_none(),
            "case {case:?}: a refused update must write none of its other members, got \
             body {:?}",
            read.body
        );
    }

    // A role this service CAN assume replaces the stored one.
    let updated = call_in(
        &router,
        REGION,
        "PUT",
        &format!("{DESTINATIONS}/{name}"),
        Some(json!({ "RoleArn": second })),
    )
    .await;
    assert_eq!(
        updated.status, 201,
        "case \"another role this service can assume\": expected HTTP 201, got {} with \
         body {:?}",
        updated.status, updated.body
    );
    let read = call_in(
        &router,
        REGION,
        "GET",
        &format!("{DESTINATIONS}/{name}"),
        None,
    )
    .await;
    assert_eq!(
        read.string("RoleArn"),
        Some(second.as_str()),
        "case \"another role this service can assume\": expected the stored RoleArn to be \
         {second:?}, got body {:?}",
        read.body
    );

    // An update that names no role changes only what it named.
    let described = call_in(
        &router,
        REGION,
        "PUT",
        &format!("{DESTINATIONS}/{name}"),
        Some(json!({ "Description": "unchanged role" })),
    )
    .await;
    assert_eq!(
        described.status, 201,
        "case \"an update naming no role\": expected HTTP 201, got {} with body {:?}",
        described.status, described.body
    );
    let read = call_in(
        &router,
        REGION,
        "GET",
        &format!("{DESTINATIONS}/{name}"),
        None,
    )
    .await;
    assert_eq!(
        read.string("RoleArn"),
        Some(second.as_str()),
        "case \"an update naming no role\": expected the stored RoleArn to be left alone, \
         got body {:?}",
        read.body
    );
    assert_eq!(
        read.string("Description"),
        Some("unchanged role"),
        "case \"an update naming no role\": expected the description it named, got body {:?}",
        read.body
    );
}
