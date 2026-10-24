// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RDS License Server endpoint slice, driven over the real
//! wire path: every call goes through the router's `dispatch`, so the request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's
//! would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `ListLicenseServerEndpoints` reports exactly the endpoints persisted by
//!   `CreateLicenseServerEndpoint`, in creation order, every member read back from
//!   what the create stored;
//! * `Filters` select on the ONE attribute the operation's model documents
//!   (`IdentityProviderArn`) and anything else is refused, whatever the store holds;
//! * `MaxResults`/`NextToken` page the list exactly once over every element, and a
//!   `MaxResults` outside the published `@range(min: 1, max: 100)` is refused;
//! * a cursor is only honoured by the list that minted it;
//! * the `PROVISIONING` -> `PROVISIONED` transition the clock owes is materialised on
//!   a later read, and the endpoint address appears only once it is provisioned;
//! * an endpoint for an identity provider that is not registered is refused and
//!   stores nothing, and a repeat create is idempotent.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suite rather than a second copy.
use crate::identity_provider_subscriptions::{
    as_str, call, harness, list_providers, provider_union, register, Harness, Reply, ACCOUNT,
    REGION, SIGNING_NAME,
};

/// The only value `ServerType` accepts (Smithy: `ServerType` enum, values `[RDS_SAL]`).
pub(crate) const SERVER_TYPE: &str = "RDS_SAL";

/// Assert `reply` is the modelled `ValidationException` (HTTP 400).
fn refused_as_validation(reply: &Reply, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {reply:?}"
    );
}

pub(crate) async fn create_endpoint(h: &Harness, body: &Value) -> Reply {
    // `CreateLicenseServerEndpoint` is bound to POST
    // /license-server/CreateLicenseServerEndpoint (smithy_query: http.uri).
    call(h, "/license-server/CreateLicenseServerEndpoint", body).await
}

pub(crate) async fn list_endpoints(h: &Harness, body: &Value) -> Reply {
    // `ListLicenseServerEndpoints` is bound to POST
    // /license-server/ListLicenseServerEndpoints (smithy_query: http.uri).
    call(h, "/license-server/ListLicenseServerEndpoints", body).await
}

/// One license server endpoint to exercise. The rows differ along what the rules
/// generalise over: the directory-id form the provider was registered with, the
/// product, the Secrets Manager secret id form, and whether tags are supplied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Endpoint {
    pub(crate) case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`.
    directory_id: &'static str,
    /// From `RegisterIdentityProvider`'s `Product` documentation: VISUAL_STUDIO_ENTERPRISE
    /// | VISUAL_STUDIO_PROFESSIONAL | OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD |
    /// REMOTE_DESKTOP_SERVICES.
    product: &'static str,
    secret_id: &'static str,
    pub(crate) tagged: bool,
}

pub(crate) const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        case: "aws-managed directory, remote desktop services, secret name",
        directory_id: "d-0123456789",
        product: "REMOTE_DESKTOP_SERVICES",
        secret_id: "rds-sal-admin",
        tagged: false,
    },
    Endpoint {
        case: "shared directory, office standard, secret arn, tagged",
        directory_id: "sd-abcdef0123",
        product: "OFFICE_STANDARD",
        secret_id: "arn:aws:secretsmanager:us-west-2:000000000000:secret:rds-sal-AbCdEf",
        tagged: true,
    },
    Endpoint {
        case: "same directory as the first row, a second product",
        directory_id: "d-0123456789",
        product: "OFFICE_PROFESSIONAL_PLUS",
        secret_id: "rds/sal/office",
        tagged: true,
    },
    Endpoint {
        case: "another directory, visual studio enterprise",
        directory_id: "d-00000000ff",
        product: "VISUAL_STUDIO_ENTERPRISE",
        secret_id: "rds-sal-vs",
        tagged: false,
    },
];

impl Endpoint {
    fn registration(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
            "Settings": {
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupId": "sg-0123abcd",
            },
        })
    }

    pub(crate) fn request(&self, identity_provider_arn: &str) -> Value {
        let mut body = json!({
            "IdentityProviderArn": identity_provider_arn,
            "LicenseServerSettings": {
                "ServerType": SERVER_TYPE,
                "ServerSettings": {
                    "RdsSalSettings": {
                        "RdsSalCredentialsProvider": {
                            "SecretsManagerCredentialsProvider": { "SecretId": self.secret_id },
                        },
                    },
                },
            },
        });
        if self.tagged {
            body["Tags"] = json!({ "team": "licensing", "case": self.case });
        }
        body
    }
}

/// The `arn:` prefix every resource of `kind` minted for this request must carry.
pub(crate) fn arn_prefix(kind: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:{kind}/")
}

/// The endpoints a list reply reports, as their ARNs in reply order.
pub(crate) fn listed_arns(body: &Value, case: &str) -> Vec<String> {
    body.get("LicenseServerEndpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: LicenseServerEndpoints is missing from {body:?}"))
        .iter()
        .map(|endpoint| as_str(endpoint, "LicenseServerEndpointArn", case).to_string())
        .collect()
}

/// Register every row's identity provider and create its endpoint, answering the
/// `(case, IdentityProviderArn, LicenseServerEndpointArn)` triples in creation order.
pub(crate) async fn seed(h: &Harness) -> Vec<(&'static str, String, String)> {
    let mut created = Vec::new();
    for row in ENDPOINTS {
        let registered = register(h, &row.registration()).await;
        let provider_arn = as_str(
            &registered.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();
        let reply = create_endpoint(h, &row.request(&provider_arn)).await;
        let body = reply.ok(row.case);
        let endpoint_arn = as_str(body, "LicenseServerEndpointArn", row.case).to_string();
        assert_eq!(
            as_str(body, "IdentityProviderArn", row.case),
            provider_arn,
            "case {:?}: the create must echo the identity provider it created for, got {body:?}",
            row.case
        );
        assert!(
            endpoint_arn.starts_with(&arn_prefix("license-server-endpoint")),
            "case {:?}: expected an ARN under {}, got {endpoint_arn:?}",
            row.case,
            arn_prefix("license-server-endpoint")
        );
        created.push((row.case, provider_arn, endpoint_arn));
    }
    created
}

/// RULE: the list reports exactly the endpoints persisted by the creates, in creation
/// order, with every member computed from the stored record.
#[tokio::test]
async fn created_endpoints_round_trip_through_the_list() {
    let h = harness();
    let created = seed(&h).await;

    let reply = list_endpoints(&h, &json!({})).await;
    let body = reply.ok("list every endpoint");
    let endpoints = body["LicenseServerEndpoints"]
        .as_array()
        .expect("LicenseServerEndpoints is reported");
    assert_eq!(
        endpoints.len(),
        created.len(),
        "the list must report every created endpoint once, got {body:?}"
    );
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "an unpaged list is complete, so it has no NextToken: {body:?}"
    );

    for (listed, (case, provider_arn, endpoint_arn)) in endpoints.iter().zip(&created) {
        assert_eq!(
            as_str(listed, "LicenseServerEndpointArn", case),
            endpoint_arn,
            "case {case:?}: the list must report the endpoints in creation order, got {listed:?}"
        );
        assert_eq!(
            as_str(listed, "IdentityProviderArn", case),
            provider_arn,
            "case {case:?}: the endpoint must carry the provider it was created for, got {listed:?}"
        );
        assert_eq!(
            as_str(listed, "ServerType", case),
            SERVER_TYPE,
            "case {case:?}: the endpoint must carry the created server type, got {listed:?}"
        );
        assert_eq!(
            as_str(listed, "LicenseServerEndpointProvisioningStatus", case),
            "PROVISIONING",
            "case {case:?}: a fresh endpoint is still provisioning, got {listed:?}"
        );
        assert!(
            listed.get("ServerEndpoint").is_none_or(Value::is_null),
            "case {case:?}: an endpoint still provisioning has no address yet, got {listed:?}"
        );
        let id = as_str(listed, "LicenseServerEndpointId", case);
        assert_eq!(
            endpoint_arn.as_str(),
            format!("{}{id}", arn_prefix("license-server-endpoint")),
            "case {case:?}: the ARN must address the reported endpoint id, got {listed:?}"
        );
        assert!(
            listed
                .get("CreationTime")
                .and_then(Value::as_f64)
                .is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: the creation instant must be reported, got {listed:?}"
        );
    }
}

/// RULE: `Filters` select on the one attribute this operation's model documents
/// (`IdentityProviderArn`), under each comparison this service implements.
#[tokio::test]
async fn the_documented_filter_selects_endpoints() {
    let h = harness();
    let created = seed(&h).await;

    for (case, provider_arn, endpoint_arn) in &created {
        // Rows differ along the comparison asked for and the selected count it implies.
        let expected_others: Vec<String> = created
            .iter()
            .filter(|(_, _, arn)| arn != endpoint_arn)
            .map(|(_, _, arn)| arn.clone())
            .collect();
        let fragment = provider_arn
            .rsplit('/')
            .next()
            .expect("the provider ARN has a resource id");
        let cases: Vec<(String, Value, Vec<String>)> = vec![
            (
                format!("{case}: documented lowercase eq"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "eq", "Value": provider_arn}),
                vec![endpoint_arn.clone()],
            ),
            (
                format!("{case}: PascalCase Equals"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "Equals", "Value": provider_arn}),
                vec![endpoint_arn.clone()],
            ),
            (
                format!("{case}: absent operation is equality"),
                json!({"Attribute": "IdentityProviderArn", "Value": provider_arn}),
                vec![endpoint_arn.clone()],
            ),
            (
                format!("{case}: NotEquals selects every other endpoint"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "NotEquals", "Value": provider_arn}),
                expected_others,
            ),
            (
                format!("{case}: Contains on the provider resource id"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "Contains", "Value": fragment}),
                vec![endpoint_arn.clone()],
            ),
            (
                format!("{case}: BeginsWith on the whole provider ARN"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "BeginsWith", "Value": provider_arn}),
                vec![endpoint_arn.clone()],
            ),
            (
                format!("{case}: Equals an ARN no endpoint carries"),
                json!({"Attribute": "IdentityProviderArn", "Operation": "Equals", "Value": format!("{provider_arn}-absent")}),
                Vec::new(),
            ),
        ];
        for (case, filter, expected) in cases {
            let reply = list_endpoints(&h, &json!({"Filters": [filter]})).await;
            let arns = listed_arns(reply.ok(&case), &case);
            assert_eq!(
                arns, expected,
                "case {case:?}: the filter must select exactly the matching endpoints, got {:?}",
                reply.body
            );
        }
    }
}

/// RULE: a filter naming an attribute this operation does not expose, or a comparison
/// this service does not implement, is refused — whatever the store holds, so an empty
/// store cannot turn a refused request into a 200.
#[tokio::test]
async fn a_filter_outside_the_documented_attribute_is_refused() {
    let h = harness();
    let cases = [
        (
            "an attribute of another list operation",
            json!({"Attribute": "Product", "Operation": "Equals", "Value": "OFFICE_STANDARD"}),
        ),
        (
            "a member of the endpoint that is not documented as filterable",
            json!({"Attribute": "ServerType", "Operation": "Equals", "Value": SERVER_TYPE}),
        ),
        (
            "the documented attribute in the wrong case",
            json!({"Attribute": "identityProviderArn", "Operation": "Equals", "Value": "x"}),
        ),
        (
            "no attribute at all",
            json!({"Operation": "Equals", "Value": "x"}),
        ),
        (
            "an unsupported comparison",
            json!({"Attribute": "IdentityProviderArn", "Operation": "geq", "Value": "x"}),
        ),
    ];

    // Empty store first: the refusal cannot depend on what is in state.
    for (case, filter) in &cases {
        let case = format!("{case} (empty store)");
        let reply = list_endpoints(&h, &json!({"Filters": [filter]})).await;
        refused_as_validation(&reply, &case);
    }

    seed(&h).await;
    for (case, filter) in &cases {
        let case = format!("{case} (populated store)");
        let reply = list_endpoints(&h, &json!({"Filters": [filter]})).await;
        refused_as_validation(&reply, &case);
    }
}

/// RULE: `MaxResults`/`NextToken` page the list exactly once over every element, for
/// every page size, and only the final page carries no cursor.
#[tokio::test]
async fn max_results_pages_every_endpoint_exactly_once() {
    let h = harness();
    let created = seed(&h).await;
    let expected: Vec<String> = created
        .iter()
        .map(|(_, _, endpoint_arn)| endpoint_arn.clone())
        .collect();

    for page_size in 1..=expected.len() + 1 {
        let case = format!("MaxResults = {page_size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, collected {collected:?}"
            );
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = list_endpoints(&h, &request).await;
            let body = reply.ok(&case);
            let arns = listed_arns(body, &case);
            assert!(
                arns.len() <= page_size,
                "case {case:?}: a page may not exceed MaxResults, got {arns:?}"
            );
            collected.extend(arns);
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: paging must report every endpoint exactly once, in creation order"
        );
    }
}

/// RULE: `MaxResults` is refused outside its published bounds and accepted on them —
/// Smithy: `ListLicenseServerEndpointsRequest$MaxResults` carries
/// `@range(min: 1, max: 100)`.
#[tokio::test]
async fn max_results_outside_the_published_range_is_refused() {
    let h = harness();
    let created = seed(&h).await;

    for max_results in [-1, 0, 101, 1_000] {
        let case = format!("MaxResults = {max_results} (outside 1..=100)");
        let reply = list_endpoints(&h, &json!({ "MaxResults": max_results })).await;
        refused_as_validation(&reply, &case);
    }

    for max_results in [1, 100] {
        let case = format!("MaxResults = {max_results} (a published bound)");
        let reply = list_endpoints(&h, &json!({ "MaxResults": max_results })).await;
        let body = reply.ok(&case);
        let arns = listed_arns(body, &case);
        assert_eq!(
            arns.len(),
            created.len().min(max_results as usize),
            "case {case:?}: a bound of the published range must be served, got {body:?}"
        );
    }
}

/// RULE: a `NextToken` is a position in ONE list — a token that another list minted, or
/// one no list minted, is refused instead of silently restarting from the beginning.
#[tokio::test]
async fn a_cursor_from_another_list_is_refused() {
    let h = harness();
    let created = seed(&h).await;
    let (_, provider_arn, _) = &created[0];

    let unfiltered = list_endpoints(&h, &json!({"MaxResults": 1})).await;
    let unfiltered_token = unfiltered.ok("unfiltered first page")["NextToken"]
        .as_str()
        .expect("a truncated list mints a cursor")
        .to_string();
    let filtered = list_endpoints(
        &h,
        &json!({
            "MaxResults": 1,
            "Filters": [{"Attribute": "IdentityProviderArn", "Operation": "NotEquals", "Value": provider_arn}],
        }),
    )
    .await;
    let filtered_token = filtered.ok("filtered first page")["NextToken"]
        .as_str()
        .expect("a truncated filtered list mints a cursor")
        .to_string();
    let providers = list_providers(&h, &json!({"MaxResults": 1})).await;
    let provider_token = providers.ok("provider first page")["NextToken"]
        .as_str()
        .expect("a truncated provider list mints a cursor")
        .to_string();

    let cases = [
        ("not hex at all", json!({"NextToken": "not-a-token"})),
        (
            "hex of a position in no named list",
            json!({"NextToken": hex::encode("1")}),
        ),
        (
            "a cursor minted by ListIdentityProviders",
            json!({"NextToken": provider_token}),
        ),
        (
            "a cursor minted by the same list under a filter",
            json!({"NextToken": filtered_token}),
        ),
        (
            "a cursor of the unfiltered list applied to a filtered one",
            json!({
                "NextToken": unfiltered_token,
                "Filters": [{"Attribute": "IdentityProviderArn", "Operation": "Equals", "Value": provider_arn}],
            }),
        ),
    ];
    for (case, request) in cases {
        let reply = list_endpoints(&h, &request).await;
        refused_as_validation(&reply, case);
    }
}

/// RULE: the `PROVISIONING` -> `PROVISIONED` transition the clock owes is materialised
/// on a later read, and the endpoint's network address is reported only once it is
/// provisioned.
#[tokio::test]
async fn the_provisioning_transition_the_clock_owes_is_materialised_on_a_later_read() {
    let h = harness();
    let created = seed(&h).await;

    let fresh = list_endpoints(&h, &json!({})).await;
    for (listed, (case, _, _)) in fresh.ok("fresh read")["LicenseServerEndpoints"]
        .as_array()
        .expect("LicenseServerEndpoints is reported")
        .iter()
        .zip(&created)
    {
        assert_eq!(
            as_str(listed, "LicenseServerEndpointProvisioningStatus", case),
            "PROVISIONING",
            "case {case:?}: a caller reading straight after the call still sees the \
             transient status, got {listed:?}"
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    let settled = list_endpoints(&h, &json!({})).await;
    for (listed, (case, _, _)) in settled.ok("settled read")["LicenseServerEndpoints"]
        .as_array()
        .expect("LicenseServerEndpoints is reported")
        .iter()
        .zip(&created)
    {
        assert_eq!(
            as_str(listed, "LicenseServerEndpointProvisioningStatus", case),
            "PROVISIONED",
            "case {case:?}: the transition the clock owed must be materialised on a \
             later read, got {listed:?}"
        );
        let id = as_str(listed, "LicenseServerEndpointId", case);
        let endpoint = listed
            .get("ServerEndpoint")
            .unwrap_or_else(|| panic!("case {case:?}: a provisioned endpoint has an address"));
        assert_eq!(
            as_str(endpoint, "Endpoint", case),
            format!("{id}.{SIGNING_NAME}.{REGION}.amazonaws.com"),
            "case {case:?}: the address must name the endpoint and its region, got {listed:?}"
        );
    }
}

/// RULE: an endpoint is created FOR a registered identity provider — an
/// `IdentityProviderArn` that resolves to none is refused as an invalid parameter and
/// stores nothing.
#[tokio::test]
async fn an_endpoint_for_an_unregistered_provider_is_refused() {
    let h = harness();
    // Every ARN below satisfies the modelled `Arn` pattern, so the refusal is the
    // service's own resolution and not the boundary validator's.
    let unregistered = [
        (
            "never registered, this account and region",
            format!("{}never-registered", arn_prefix("identity-provider")),
        ),
        (
            "another region",
            format!("arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT}:identity-provider/ip-000000000001"),
        ),
        (
            "another account",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:210987654321:identity-provider/ip-000000000001"),
        ),
        (
            "another service's resource",
            format!("arn:aws:license-manager:{REGION}:{ACCOUNT}:license/l-000000000001"),
        ),
    ];
    for (case, provider_arn) in &unregistered {
        let reply = create_endpoint(&h, &ENDPOINTS[0].request(provider_arn)).await;
        refused_as_validation(&reply, case);
    }

    let reply = list_endpoints(&h, &json!({})).await;
    assert_eq!(
        listed_arns(reply.ok("nothing created"), "nothing created"),
        Vec::<String>::new(),
        "a refused create must store nothing, got {:?}",
        reply.body
    );
}

/// RULE: `CreateLicenseServerEndpoint` is `@idempotent` — a repeat create for the same
/// identity provider and server type answers with the endpoint already in state instead
/// of minting a second one.
#[tokio::test]
async fn a_repeat_create_answers_with_the_endpoint_already_in_state() {
    let h = harness();
    let created = seed(&h).await;

    for (row, (case, provider_arn, endpoint_arn)) in ENDPOINTS.iter().zip(&created) {
        let again = create_endpoint(&h, &row.request(provider_arn)).await;
        let body = again.ok(case);
        assert_eq!(
            as_str(body, "LicenseServerEndpointArn", case),
            endpoint_arn,
            "case {case:?}: a repeat create must answer with the stored endpoint, got {body:?}"
        );
    }

    let reply = list_endpoints(&h, &json!({})).await;
    let expected: Vec<String> = created
        .iter()
        .map(|(_, _, endpoint_arn)| endpoint_arn.clone())
        .collect();
    assert_eq!(
        listed_arns(reply.ok("no duplicates"), "no duplicates"),
        expected,
        "a repeat create must not add a record, got {:?}",
        reply.body
    );
}

/// RULE: a value outside a modelled constraint is refused before the handler runs —
/// `ServerType` is an enum whose only value is `RDS_SAL`, and both
/// `IdentityProviderArn` and `LicenseServerSettings` are `@required`.
#[tokio::test]
async fn a_create_outside_the_modelled_constraints_is_refused() {
    let h = harness();
    let created = seed(&h).await;
    let (_, provider_arn, _) = &created[0];
    let settings = |server_type: &str| {
        json!({
            "ServerType": server_type,
            "ServerSettings": {
                "RdsSalSettings": {
                    "RdsSalCredentialsProvider": {
                        "SecretsManagerCredentialsProvider": { "SecretId": "rds-sal-admin" },
                    },
                },
            },
        })
    };
    let cases = [
        (
            "a ServerType outside the modelled enum",
            json!({"IdentityProviderArn": provider_arn, "LicenseServerSettings": settings("RDS_SAL_V2")}),
        ),
        (
            "an empty ServerType",
            json!({"IdentityProviderArn": provider_arn, "LicenseServerSettings": settings("")}),
        ),
        (
            "no LicenseServerSettings at all (@required)",
            json!({"IdentityProviderArn": provider_arn}),
        ),
        (
            "no IdentityProviderArn at all (@required)",
            json!({"LicenseServerSettings": settings(SERVER_TYPE)}),
        ),
        (
            "an IdentityProviderArn outside the modelled pattern",
            json!({"IdentityProviderArn": "not-an-arn", "LicenseServerSettings": settings(SERVER_TYPE)}),
        ),
    ];
    for (case, request) in cases {
        let reply = create_endpoint(&h, &request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
    }

    let reply = list_endpoints(&h, &json!({})).await;
    assert_eq!(
        listed_arns(reply.ok("nothing added"), "nothing added").len(),
        created.len(),
        "a refused create must not add a record, got {:?}",
        reply.body
    );
}
