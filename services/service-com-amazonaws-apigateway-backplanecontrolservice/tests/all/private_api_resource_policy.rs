// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a REST API's RESOURCE POLICY on the `execute-api` data plane: which
//! requests to a deployed stage the api's own `policy` admits, and which it refuses.
//!
//! `RestApi$policy` is "a stringified JSON policy document that applies to this RestApi
//! regardless of the caller and Method configuration", so the rule under test is: a document
//! the developer attached decides EVERY request the endpoint answers — a request it allows
//! is served by the method's integration, and one it does not is refused `403` with the
//! `execute-api:Invoke` authorization sentence, with the integration never reached. A
//! PRIVATE api pinned to one of its interface VPC endpoints with a `StringEquals`
//! `aws:SourceVpce` condition is the case that rule exists for: the request that arrived
//! through the named endpoint is served and the one that arrived through another is not.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, on the api's own
//! `{rest-api-id}.execute-api.{region}.amazonaws.com` host or on the endpoint-specific
//! `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com` host EC2 reports in an
//! interface endpoint's `DnsEntries` — and the records are written through the control plane
//! (`CreateRestApi`, `PutMethod`, `PutIntegration`, `UpdateRestApi`, `CreateDeployment`), so
//! nothing here reaches into state.
//!
//! The tables span what the rule generalises over: the EFFECT a document states about the
//! call (allow, deny, nothing), the two spellings its `Resource` element names a method by
//! (the method ARN and the `execute-api:/{stage}/{verb}/{path}` form), the ACTION and the
//! PRINCIPAL it names, the condition key it is written over (one this endpoint can answer
//! for and one it cannot) and the endpoint the request arrives through (the named one,
//! another, none at all).

use serde_json::{json, Value};

use crate::execute_api_data_plane::{invoke_on_host, json_template, mock_resource};
use crate::rest_api_lifecycle::{string_member, Gateway};

/// The Region every host in this suite is addressed in.
const REGION: &str = "us-west-2";

/// The Region the api is CREATED in — the Region the shared control-plane harness
/// ([`Gateway`]) scopes its requests to — which is therefore the Region of the method ARN a
/// statement names and a refusal reports: an api lives in one Region, so its ARNs name that
/// one whatever endpoint a later invoke arrived at (and an invoke can arrive at an endpoint
/// that names no Region at all).
const API_REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "s";

/// The resource the method under test is put on.
const RESOURCE_PATH: &str = "whoami";

/// The one value the `MOCK` integration's response template carries — what proves a
/// request reached the integration, so a refusal must never carry it.
const NONCE: &str = "a3f9c1";

/// The body that integration answers with: the nonce, as the response template writes it.
fn served_body() -> String {
    format!("{{\"m\":\"{NONCE}\"}}")
}

/// The principal a refusal names for a request to a method that authorizes nobody, which is
/// what `authorizationType` `NONE` makes every caller — signed or not.
const ANONYMOUS: &str = "anonymous";

/// A SigV4 credential scope, as a signed SDK request carries it. The signature itself is
/// the emulator's business and is not verified in these tests (the harness runs the router
/// with request authentication off), so only its PRESENCE is what a case varies.
const SIGNED: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/execute-api/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The interface VPC endpoint the policy under test pins the api to, in EC2's own id form
/// (`vpce-` and 17 lowercase hexadecimal digits).
const ENDPOINT: &str = "vpce-0123456789abcdef0";

/// Another endpoint of the same account: the one a request must NOT be admitted through
/// while the policy names [`ENDPOINT`].
const OTHER_ENDPOINT: &str = "vpce-00000000000000000";

/// The api's own invoke host, which names the api and no VPC endpoint.
fn api_host(api: &str) -> String {
    format!("{api}.execute-api.{REGION}.amazonaws.com")
}

/// The endpoint-specific DNS name of `endpoint`, the regional one EC2 reports in an
/// interface endpoint's `DnsEntries`: it names the ENDPOINT, so the api travels in the
/// `x-apigw-api-id` header AWS documents for this addressing.
fn endpoint_host(endpoint: &str) -> String {
    format!("{endpoint}.execute-api.{REGION}.vpce.amazonaws.com")
}

/// The ZONAL name of the same endpoint, which EC2 reports beside the regional one: one per
/// Availability Zone the endpoint's interfaces sit in.
fn zonal_endpoint_host(endpoint: &str) -> String {
    format!("{endpoint}-{REGION}a.execute-api.{REGION}.vpce.amazonaws.com")
}

/// How a request addresses the api: on the api's own host, or through one VPC endpoint's own
/// DNS name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The api's own `execute-api` host, which names no endpoint.
    OwnHost,
    /// The regional endpoint-specific name of the endpoint whose id this carries.
    Endpoint(&'static str),
    /// The zonal endpoint-specific name of the same endpoint.
    ZonalEndpoint(&'static str),
}

/// One PRIVATE api with a deployed `MOCK` GET method, as the control plane writes it.
struct Deployed {
    /// The api id.
    api: String,
    /// The api's root resource id, which a further resource is created under.
    root: String,
}

impl Deployed {
    /// A PRIVATE api carrying `GET /whoami`, answered by a `MOCK` integration with
    /// [`served_body`], deployed to [`STAGE`].
    ///
    /// Created PRIVATE because that is the endpoint type this rule exists for — a private
    /// api is reached through an interface VPC endpoint — though the gate itself is the
    /// document, which "applies to this RestApi regardless of the caller and Method
    /// configuration" whatever the endpoint type is.
    async fn stand_up(gateway: &Gateway, name: &str) -> Self {
        let created = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({
                    "name": name,
                    "endpointConfiguration": { "types": ["PRIVATE"] },
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case CreateRestApi({name}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        let api = string_member(&created.body, "id", name);
        let root = string_member(&created.body, "rootResourceId", name);
        mock_resource(
            gateway,
            &api,
            &root,
            RESOURCE_PATH,
            "GET",
            "200",
            json_template(&served_body()),
        )
        .await;
        crate::execute_api_data_plane::deploy(gateway, &api, STAGE).await;
        Self { api, root }
    }

    /// One more `GET` resource under the root, on a `MOCK` integration answering the same
    /// body, whose method carries `authorization_type`; answers its path part.
    ///
    /// Deployed, so the stage serves it: what the data plane answers is read out of the
    /// DEPLOYMENT.
    async fn mock_method(
        &self,
        gateway: &Gateway,
        path_part: &str,
        authorization_type: &str,
    ) -> String {
        let resource =
            crate::rest_api_lifecycle::resource_under(gateway, &self.api, &self.root, path_part)
                .await;
        crate::execute_api_data_plane::put_method(
            gateway,
            &self.api,
            &resource,
            "GET",
            json!({ "authorizationType": authorization_type }),
        )
        .await;
        crate::execute_api_data_plane::put_integration(
            gateway,
            &self.api,
            &resource,
            "GET",
            json!({ "type": "MOCK" }),
        )
        .await;
        crate::execute_api_data_plane::method_response(
            gateway,
            &self.api,
            &resource,
            "GET",
            "200",
            json!({}),
        )
        .await;
        crate::execute_api_data_plane::integration_response(
            gateway,
            &self.api,
            &resource,
            "GET",
            "200",
            json!({ "responseTemplates": json_template(&served_body()) }),
        )
        .await;
        crate::execute_api_data_plane::deploy(gateway, &self.api, STAGE).await;
        path_part.to_owned()
    }

    /// The method ARN of the deployed `GET /whoami`, as a statement of the policy may name
    /// it and as a refusal reports it.
    fn method_arn(&self) -> String {
        self.path_method_arn(RESOURCE_PATH)
    }

    /// The method ARN of the deployed `GET` on `path_part`.
    fn path_method_arn(&self, path_part: &str) -> String {
        format!(
            "arn:aws:execute-api:{API_REGION}:000000000000:{api}/{STAGE}/GET/{path_part}",
            api = self.api
        )
    }

    /// `UpdateRestApi` replacing `/policy` with `document` (or removing it for `None`),
    /// then `CreateDeployment` so the stage serves the api as it now stands. The stored
    /// document is read back through `GetRestApi`, so each case proves the round trip it
    /// then invokes against.
    async fn set_policy(&self, gateway: &Gateway, case: &str, document: Option<&Value>) {
        let stringified = document.map(::std::string::ToString::to_string);
        let patch = match &stringified {
            Some(document) => json!({
                "patchOperations": [
                    { "op": "replace", "path": "/policy", "value": document }
                ]
            }),
            None => json!({
                "patchOperations": [{ "op": "remove", "path": "/policy" }]
            }),
        };
        let patched = gateway
            .call("PATCH", &format!("/restapis/{}", self.api), Some(patch))
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected UpdateRestApi to answer 200, got {} with {:?}",
            patched.status, patched.body
        );
        let read = gateway
            .call("GET", &format!("/restapis/{}", self.api), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetRestApi to answer 200, got {} with {:?}",
            read.status, read.body
        );
        match &stringified {
            Some(document) => assert_eq!(
                string_member(&read.body, "policy", case),
                *document,
                "case {case}: expected GetRestApi to report the policy that was set"
            ),
            None => assert!(
                read.body.get("policy").is_none_or(Value::is_null),
                "case {case}: expected no policy on the api, got {:?}",
                read.body
            ),
        }
        crate::execute_api_data_plane::deploy(gateway, &self.api, STAGE).await;
    }

    /// Invoke `GET /{stage}/whoami` through `address`, answering the status and the body.
    async fn invoke(&self, gateway: &Gateway, address: Address) -> (u16, String) {
        let (host, headers): (String, Vec<(&str, &str)>) = match address {
            Address::OwnHost => (api_host(&self.api), Vec::new()),
            Address::Endpoint(endpoint) => (
                endpoint_host(endpoint),
                vec![("x-apigw-api-id", self.api.as_str())],
            ),
            Address::ZonalEndpoint(endpoint) => (
                zonal_endpoint_host(endpoint),
                vec![("x-apigw-api-id", self.api.as_str())],
            ),
        };
        let answered = invoke_on_host(
            gateway,
            &host,
            "GET",
            &format!("/{STAGE}/{RESOURCE_PATH}"),
            &headers,
        )
        .await;
        (answered.status, answered.body)
    }
}

/// The `Message` member of a refusal body — the member API Gateway answers an
/// authorization refusal with.
fn authorization_message(body: &str, case: &str) -> String {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|_| panic!("case {case}: expected a JSON body, got {body:?}"));
    parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Message member, got {body:?}"))
        .to_owned()
}

/// Assert that `address` is SERVED the integration's own body.
fn assert_served(case: &str, status: u16, body: &str) {
    assert_eq!(
        status, 200,
        "case {case}: expected the integration's 200, got {status} with {body:?}"
    );
    assert_eq!(
        body,
        served_body(),
        "case {case}: expected the integration's body, got {body:?}"
    );
}

/// Assert that the request was refused as unauthorized: `403`, the authorization sentence
/// naming the principal, the action and the method ARN, and NOTHING of the integration's
/// body — a refused request never reached it.
fn assert_refused(case: &str, status: u16, body: &str, principal: &str, method_arn: &str) {
    assert_eq!(
        status, 403,
        "case {case}: expected 403, got {status} with {body:?}"
    );
    let message = authorization_message(body, case);
    assert_eq!(
        message,
        format!(
            "User: {principal} is not authorized to perform: execute-api:Invoke on resource: \
             {method_arn}"
        ),
        "case {case}: expected the execute-api:Invoke authorization sentence, got {message:?}"
    );
    assert!(
        !body.contains(NONCE),
        "case {case}: a refused request must carry nothing of the integration's body, got \
         {body:?}"
    );
}

/// An `Allow` of `execute-api:Invoke` for every principal, over `resource`, under
/// `condition` when one is given.
fn statement(effect: &str, principal: Value, action: &str, resource: &str, condition: Option<Value>) -> Value {
    let mut entry = json!({
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        entry["Condition"] = condition;
    }
    entry
}

/// A policy document carrying `statements`.
fn policy(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// The `StringEquals` clause over the source VPC endpoint that pins an api to `endpoint`.
fn source_vpce_is(endpoint: &str) -> Value {
    json!({ "StringEquals": { "aws:SourceVpce": endpoint } })
}

#[tokio::test]
async fn the_apis_resource_policy_decides_every_invoke_of_its_deployed_stage() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "private-policy-gate").await;
    let method_arn = deployed.method_arn();

    // Rows differ along the effect the document states about the call, the spelling of the
    // resource it names, the action and principal it names, the condition key it is written
    // over, and the endpoint the request arrives through — everything the rule generalises
    // over. `None` is an api carrying no document at all, which is not gated.
    type Row<'a> = (&'a str, Option<Value>, Address, bool);
    let rows: Vec<Row<'_>> = vec![
        (
            "no policy at all",
            None,
            Address::OwnHost,
            true,
        ),
        (
            "an unconditional allow, in the scoped resource spelling",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                None,
            )])),
            Address::OwnHost,
            true,
        ),
        (
            "an unconditional allow naming the method's own ARN",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                &method_arn,
                None,
            )])),
            Address::OwnHost,
            true,
        ),
        (
            "an unconditional allow naming a wildcard ARN of every GET",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "arn:aws:execute-api:*:*:*/*/GET/*",
                None,
            )])),
            Address::OwnHost,
            true,
        ),
        (
            "an allow pinned to the endpoint, reached through that endpoint",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                Some(source_vpce_is(ENDPOINT)),
            )])),
            Address::Endpoint(ENDPOINT),
            true,
        ),
        (
            "an allow pinned to the endpoint, reached through that endpoint's zonal name",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                Some(source_vpce_is(ENDPOINT)),
            )])),
            Address::ZonalEndpoint(ENDPOINT),
            true,
        ),
        (
            "an allow pinned to the endpoint, reached through ANOTHER endpoint",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                Some(source_vpce_is(ENDPOINT)),
            )])),
            Address::Endpoint(OTHER_ENDPOINT),
            false,
        ),
        (
            "an allow pinned to the endpoint, reached through no endpoint at all",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                Some(source_vpce_is(ENDPOINT)),
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow beside a deny of every other endpoint, through the named one",
            Some(policy(vec![
                statement(
                    "Allow",
                    json!("*"),
                    "execute-api:Invoke",
                    "execute-api:/*",
                    None,
                ),
                statement(
                    "Deny",
                    json!("*"),
                    "execute-api:Invoke",
                    "execute-api:/*",
                    Some(json!({ "StringNotEquals": { "aws:SourceVpce": ENDPOINT } })),
                ),
            ])),
            Address::Endpoint(ENDPOINT),
            true,
        ),
        (
            "an allow beside a deny of every other endpoint, through another one",
            Some(policy(vec![
                statement(
                    "Allow",
                    json!("*"),
                    "execute-api:Invoke",
                    "execute-api:/*",
                    None,
                ),
                statement(
                    "Deny",
                    json!("*"),
                    "execute-api:Invoke",
                    "execute-api:/*",
                    Some(json!({ "StringNotEquals": { "aws:SourceVpce": ENDPOINT } })),
                ),
            ])),
            Address::Endpoint(OTHER_ENDPOINT),
            false,
        ),
        (
            "an explicit deny of everything",
            Some(policy(vec![statement(
                "Deny",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                None,
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow of ANOTHER resource path",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                &format!("execute-api:/{STAGE}/GET/elsewhere"),
                None,
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow of another VERB of the same path",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                &format!("execute-api:/{STAGE}/POST/{RESOURCE_PATH}"),
                None,
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow of another ACTION",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:ManageConnections",
                "execute-api:/*",
                None,
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow naming one principal rather than every one",
            Some(policy(vec![statement(
                "Allow",
                json!({ "AWS": "arn:aws:iam::000000000000:role/callers" }),
                "execute-api:Invoke",
                "execute-api:/*",
                None,
            )])),
            Address::OwnHost,
            false,
        ),
        (
            "an allow conditioned on a key this endpoint cannot answer for",
            Some(policy(vec![statement(
                "Allow",
                json!("*"),
                "execute-api:Invoke",
                "execute-api:/*",
                Some(json!({ "StringEquals": { "aws:SourceVpc": "vpc-0123456789abcdef0" } })),
            )])),
            Address::Endpoint(ENDPOINT),
            false,
        ),
        (
            "a document stating nothing at all",
            Some(policy(Vec::new())),
            Address::OwnHost,
            false,
        ),
    ];

    for (case, document, address, served) in &rows {
        deployed.set_policy(&gateway, case, document.as_ref()).await;
        let (status, body) = deployed.invoke(&gateway, *address).await;
        if *served {
            assert_served(case, status, &body);
        } else {
            assert_refused(case, status, &body, ANONYMOUS, &method_arn);
        }
    }
}

#[tokio::test]
async fn re_pinning_the_policy_to_another_endpoint_closes_the_stage_and_restoring_it_opens_it() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "private-policy-repin").await;
    let method_arn = deployed.method_arn();

    // The sequence a developer drives: the api is pinned to the endpoint in use, then to
    // another, then back — with a deployment after each change. Each step is decided by the
    // document in force at the instant of the call, so the SAME request is served, refused
    // and served again.
    let steps: &[(&str, &str, bool)] = &[
        ("pinned to the endpoint in use", ENDPOINT, true),
        ("re-pinned to an endpoint not in use", OTHER_ENDPOINT, false),
        ("pinned back to the endpoint in use", ENDPOINT, true),
    ];
    for (case, pinned, served) in steps {
        let document = policy(vec![statement(
            "Allow",
            json!("*"),
            "execute-api:Invoke",
            "execute-api:/*",
            Some(source_vpce_is(pinned)),
        )]);
        deployed.set_policy(&gateway, case, Some(&document)).await;
        let (status, body) = deployed
            .invoke(&gateway, Address::Endpoint(ENDPOINT))
            .await;
        if *served {
            assert_served(case, status, &body);
        } else {
            assert_refused(case, status, &body, ANONYMOUS, &method_arn);
        }
    }
}

#[tokio::test]
async fn an_endpoints_own_dns_name_serves_only_the_api_the_request_names() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "private-endpoint-addressing").await;
    let allow_all = policy(vec![statement(
        "Allow",
        json!("*"),
        "execute-api:Invoke",
        "execute-api:/*",
        None,
    )]);
    deployed
        .set_policy(&gateway, "the addressing cases", Some(&allow_all))
        .await;

    // Rows differ along the api id the request names in the header: an endpoint's own DNS
    // name names no api, so a request naming none, or naming one this service does not
    // hold, is claimed by nothing and gets the front door's own refusal rather than this
    // api's answer.
    let path = format!("/{STAGE}/{RESOURCE_PATH}");
    let host = endpoint_host(ENDPOINT);
    let cases: &[(&str, Option<&str>, u16)] = &[
        ("the api this service holds", Some(deployed.api.as_str()), 200),
        ("no api id at all", None, 404),
        ("an api id this service does not hold", Some("zzzzzzzzzz"), 404),
    ];
    for (case, api_id, status) in cases {
        let headers: Vec<(&str, &str)> = api_id
            .map(|api_id| vec![("x-apigw-api-id", api_id)])
            .unwrap_or_default();
        let answered = invoke_on_host(&gateway, &host, "GET", &path, &headers).await;
        assert_eq!(
            answered.status, *status,
            "case {case}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 200 {
            assert_eq!(
                answered.body,
                served_body(),
                "case {case}: expected the integration's body, got {:?}",
                answered.body
            );
        }
    }
}

#[tokio::test]
async fn the_methods_own_authorization_decides_how_an_unstated_document_is_read() {
    let gateway = Gateway::new();
    let deployed = Deployed::stand_up(&gateway, "private-policy-authorization").await;
    // A second resource the document says nothing about, on each authorization type whose
    // decision this endpoint can make: `NONE`, for which API Gateway authenticates nobody,
    // and `AWS_IAM`, for which the emulator has verified a credential by the time the
    // document is read.
    let open = deployed.mock_method(&gateway, "elsewhere", "NONE").await;
    let signed = deployed.mock_method(&gateway, "by-iam", "AWS_IAM").await;
    let granted = policy(vec![statement(
        "Allow",
        json!("*"),
        "execute-api:Invoke",
        &format!("execute-api:/{STAGE}/GET/{RESOURCE_PATH}"),
        None,
    )]);
    deployed
        .set_policy(&gateway, "one path allowed", Some(&granted))
        .await;

    // Rows differ along the path the request addresses (one the document allows, two it
    // says nothing about), the method's authorization type, and whether the request carries
    // a credential — which must NOT be what opens a gate the document leaves shut.
    let iam_principal = ::service_common::caller_principal::ambient_user_arn();
    type Row<'a> = (&'a str, &'a str, bool, bool);
    let rows: &[Row<'_>] = &[
        ("the allowed path, unsigned", RESOURCE_PATH, false, true),
        ("the allowed path, signed", RESOURCE_PATH, true, true),
        ("an unstated path on an open method, unsigned", &open, false, false),
        // A credential does not open what the document leaves shut: the method authorizes
        // nobody, so the request still speaks for nobody.
        ("an unstated path on an open method, signed", &open, true, false),
        // An `AWS_IAM` method is one AWS decides against the caller's identity-based
        // policies as well, which this emulator does not model — so an unstated document
        // does not refuse it.
        ("an unstated path on an AWS_IAM method", &signed, true, true),
    ];
    for (case, path, sign, served) in rows {
        let headers: Vec<(&str, &str)> = if *sign {
            vec![("authorization", SIGNED)]
        } else {
            Vec::new()
        };
        let answered = invoke_on_host(
            &gateway,
            &api_host(&deployed.api),
            "GET",
            &format!("/{STAGE}/{path}"),
            &headers,
        )
        .await;
        if *served {
            assert_served(case, answered.status, &answered.body);
        } else {
            assert_refused(
                case,
                answered.status,
                &answered.body,
                ANONYMOUS,
                &deployed.path_method_arn(path),
            );
        }
    }

    // An explicit deny refuses whoever asked, the IAM-authorized caller included — and the
    // sentence then names the principal the request spoke for rather than nobody.
    let denied = policy(vec![statement(
        "Deny",
        json!("*"),
        "execute-api:Invoke",
        "execute-api:/*",
        None,
    )]);
    deployed
        .set_policy(&gateway, "an explicit deny of everything", Some(&denied))
        .await;
    let answered = invoke_on_host(
        &gateway,
        &api_host(&deployed.api),
        "GET",
        &format!("/{STAGE}/{signed}"),
        &[("authorization", SIGNED)],
    )
    .await;
    assert_refused(
        "an explicit deny reaching an AWS_IAM method",
        answered.status,
        &answered.body,
        &iam_principal,
        &deployed.path_method_arn(&signed),
    );
}
