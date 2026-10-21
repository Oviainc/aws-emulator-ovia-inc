// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the subscription-provider registration lifecycle and the
//! service-settings pair, driven over the real wire path: every call goes through the
//! router's `dispatch`, so the request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * every operation answers from the single shared store — what a registration
//!   persisted is what a later Get/List reports, and what a deregistration removed is
//!   gone for every later caller;
//! * a registration gets a minted `SubscriptionProviderArn` of the modelled shape,
//!   recorded in state, and re-registering the same (source, secret ARN) is idempotent;
//! * `MaxResults`/`NextToken` page the list exactly once over every element, in
//!   registration order, with no token on the last page;
//! * `SubscriptionProviderSources` selects on the stored source;
//! * `UpdateServiceSettings` persists the settings singleton that `GetServiceSettings`
//!   reports, and `AllowUpdate` is what admits a second home Region.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    SubscriptionProviderRecord, MAX_PROVIDERS,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::{
    ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState,
    EmulatedService,
};

/// This service's sigv4 signing name (`OPERATIONS.md`, "Wire protocol").
const SIGNING_NAME: &str = "license-manager-linux-subscriptions";
/// The region the test requests are signed for; every minted ARN must carry it.
const REGION: &str = "us-west-2";
/// The canonical account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The only `SubscriptionProviderSource` the model declares (`one of: RedHat`).
const RED_HAT: &str = "RedHat";

/// Secret ARNs spanning the modelled `SecretArn` grammar
/// `^arn:[a-z0-9-\.]{1,63}:secretsmanager:[a-z0-9-\.]{0,63}:[a-z0-9-\.]{0,63}:secret:[^/]{1,1023}$`:
/// a plain one, another partition, empty region and account (the `{0,63}` segments), and
/// a secret name using characters the `[^/]` class allows.
const SECRET_ARNS: &[&str] = &[
    "arn:aws:secretsmanager:us-east-1:123456789012:secret:rhel-token",
    "arn:aws-us-gov:secretsmanager:us-gov-west-1:123456789012:secret:rhsm-offline",
    "arn:aws:secretsmanager:::secret:token",
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:RedHat_Offline+Token=1",
];

/// The values `LinuxSubscriptionsDiscovery` and `OrganizationIntegration` accept
/// (`shared_types`: `Disabled | Enabled`).
const ON_OFF: &[&str] = &["Disabled", "Enabled"];

/// One emulator: the router that dispatches, plus the shared engine its service was
/// built from (so a test can read or pre-fill the store the handlers use).
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests sign nothing; authentication is the framework's concern, not this
    // service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

/// What a call answered: the HTTP status, the wire error code (if any) and the body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert this reply is the modelled client error `code` with HTTP `status`.
    fn error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            self.status, self.body
        );
        let actual = self.error_type.as_deref().unwrap_or("<none>");
        assert!(
            actual.ends_with(code),
            "case {case:?}: expected error code {code}, got {actual:?} with body {:?}",
            self.body
        );
    }
}

async fn call_in_region(h: &Harness, region: &str, path: &str, body: &Value) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{SIGNING_NAME}.{region}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{region}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_type,
        body,
    }
}

async fn call(h: &Harness, path: &str, body: &Value) -> Reply {
    call_in_region(h, REGION, path, body).await
}

async fn register(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/RegisterSubscriptionProvider", body).await
}

async fn get_provider(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/GetRegisteredSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

async fn list_providers(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/ListRegisteredSubscriptionProviders", body).await
}

async fn deregister(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/DeregisterSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

async fn get_settings(h: &Harness) -> Reply {
    call(h, "/subscription/GetServiceSettings", &json!({})).await
}

async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/UpdateServiceSettings", body).await
}

/// The `SubscriptionProviderArn` member of a 200 body.
fn provider_arn(reply: &Reply, case: &str) -> String {
    reply
        .ok(case)
        .get("SubscriptionProviderArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionProviderArn in {:?}", reply.body))
        .to_string()
}

/// Assert `arn` satisfies the modelled `SubscriptionProviderArn` pattern
/// `^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}$`
/// and names this service, region and account.
fn assert_provider_arn(arn: &str, case: &str) {
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: expected a 6-segment ARN, got {arn:?}"
    );
    assert_eq!(
        &segments[0..5],
        &["arn", "aws", SIGNING_NAME, REGION, ACCOUNT],
        "case {case:?}: unexpected ARN prefix in {arn:?}"
    );
    let (kind, id) = segments[5]
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: resource segment has no '/' in {arn:?}"));
    assert_eq!(
        kind, "subscription-provider",
        "case {case:?}: unexpected resource kind in {arn:?}"
    );
    assert!(
        !id.is_empty()
            && id.len() <= 510
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'),
        "case {case:?}: resource id {id:?} is outside [a-z0-9-.]{{1,510}}"
    );
}

/// The providers a list body reports, as `(arn, source, secret arn, status)` rows.
fn listed(reply: &Reply, case: &str) -> Vec<(String, String, String, String)> {
    reply
        .ok(case)
        .get("RegisteredSubscriptionProviders")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no RegisteredSubscriptionProviders in {:?}",
                reply.body
            )
        })
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
                    .to_string()
            };
            (
                member("SubscriptionProviderArn"),
                member("SubscriptionProviderSource"),
                member("SecretArn"),
                member("SubscriptionProviderStatus"),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_registration_is_persisted_and_read_back_by_get_and_list() {
    let h = harness();
    let mut expected = Vec::new();

    for secret_arn in SECRET_ARNS {
        let case = format!("register {secret_arn}");
        let reply = register(
            &h,
            &json!({
                "SubscriptionProviderSource": RED_HAT,
                "SecretArn": secret_arn,
                "Tags": { "owner": "platform" },
            }),
        )
        .await;
        let body = reply.ok(&case);
        assert_eq!(
            body.get("SubscriptionProviderSource").and_then(Value::as_str),
            Some(RED_HAT),
            "case {case:?}: expected the registered source back, got {body:?}"
        );
        assert_eq!(
            body.get("SubscriptionProviderStatus").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: expected SubscriptionProviderStatus ACTIVE, got {body:?}"
        );
        let arn = provider_arn(&reply, &case);
        assert_provider_arn(&arn, &case);
        assert!(
            !expected.iter().any(|(stored, _): &(String, String)| stored == &arn),
            "case {case:?}: minted ARN {arn:?} collides with an earlier registration"
        );

        // Round trip: the registration is readable through Get, member for member.
        let got = get_provider(&h, &arn).await;
        let body = got.ok(&case);
        for (member, want) in [
            ("SubscriptionProviderArn", arn.as_str()),
            ("SubscriptionProviderSource", RED_HAT),
            ("SecretArn", secret_arn),
            ("SubscriptionProviderStatus", "ACTIVE"),
        ] {
            assert_eq!(
                body.get(member).and_then(Value::as_str),
                Some(want),
                "case {case:?}: expected {member} {want:?}, got {body:?}"
            );
        }
        // Nothing was retrieved from the third-party provider and nothing is wrong with
        // the token, so neither member is invented.
        for absent in [
            "SubscriptionProviderStatusMessage",
            "LastSuccessfulDataRetrievalTime",
        ] {
            assert!(
                body.get(absent).is_none_or(Value::is_null),
                "case {case:?}: expected no {absent}, got {body:?}"
            );
        }
        expected.push((arn, (*secret_arn).to_string()));
    }

    // The list reports every registration, only those, in registration order.
    let case = "list after registering every row";
    let rows = listed(&list_providers(&h, &json!({})).await, case);
    assert_eq!(
        rows.iter()
            .map(|(arn, _, secret, _)| (arn.clone(), secret.clone()))
            .collect::<Vec<_>>(),
        expected,
        "case {case:?}: the list must report exactly the registrations, in order"
    );
    for (arn, source, _, status) in &rows {
        assert_eq!(
            (source.as_str(), status.as_str()),
            (RED_HAT, "ACTIVE"),
            "case {case:?}: unexpected source/status for {arn}"
        );
    }
}

#[tokio::test]
async fn re_registering_the_same_provider_is_idempotent() {
    let h = harness();
    let mut arns = Vec::new();
    for secret_arn in SECRET_ARNS {
        let case = format!("first registration of {secret_arn}");
        let first = provider_arn(
            &register(
                &h,
                &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
            )
            .await,
            &case,
        );
        arns.push(first);
    }

    for (index, secret_arn) in SECRET_ARNS.iter().enumerate() {
        let case = format!("re-registration of {secret_arn}");
        // A repeat differing only in the optional Tags is still the same provider.
        let again = provider_arn(
            &register(
                &h,
                &json!({
                    "SubscriptionProviderSource": RED_HAT,
                    "SecretArn": secret_arn,
                    "Tags": { "round": "two" },
                }),
            )
            .await,
            &case,
        );
        assert_eq!(
            again, arns[index],
            "case {case:?}: expected the ARN of the registration already in state"
        );
    }

    let case = "list after re-registering every row";
    let rows = listed(&list_providers(&h, &json!({})).await, case);
    assert_eq!(
        rows.len(),
        SECRET_ARNS.len(),
        "case {case:?}: expected {} registrations, got {rows:?}",
        SECRET_ARNS.len()
    );
}

#[tokio::test]
async fn the_list_pages_exactly_once_over_every_registration() {
    let h = harness();
    let mut expected = Vec::new();
    for secret_arn in SECRET_ARNS {
        let case = format!("register {secret_arn}");
        expected.push(provider_arn(
            &register(
                &h,
                &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
            )
            .await,
            &case,
        ));
    }

    // Every page size from 1 to one past the element count: the pages concatenate to the
    // full list in order, and only the last page omits the token.
    for page_size in 1..=expected.len() + 1 {
        let case = format!("MaxResults {page_size}");
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = list_providers(&h, &request).await;
            let page = listed(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: page of {} exceeds MaxResults, got {page:?}",
                page.len()
            );
            seen.extend(page.into_iter().map(|(arn, _, _, _)| arn));
            token = reply
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must cover every registration exactly once, in order"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {case:?}: unexpected page count for {} elements",
            expected.len()
        );
    }

    // A cursor is minted by one list and refused elsewhere: a token that was not minted
    // at all is not a position in any list.
    let case = "forged NextToken";
    list_providers(&h, &json!({ "NextToken": "not-a-cursor" }))
        .await
        .error(case, 400, "ValidationException");
}

#[tokio::test]
async fn the_source_selection_narrows_the_list_to_the_stored_source() {
    let h = harness();
    for secret_arn in SECRET_ARNS {
        let case = format!("register {secret_arn}");
        register(
            &h,
            &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
        )
        .await
        .ok(&case);
    }

    // An absent or empty selection selects every source; naming the stored source
    // selects the same set.
    for (case, request) in [
        ("no selection", json!({})),
        ("empty selection", json!({ "SubscriptionProviderSources": [] })),
        (
            "RedHat selected",
            json!({ "SubscriptionProviderSources": [RED_HAT] }),
        ),
    ] {
        let rows = listed(&list_providers(&h, &request).await, case);
        assert_eq!(
            rows.len(),
            SECRET_ARNS.len(),
            "case {case:?}: expected every registration, got {rows:?}"
        );
    }

    // A value outside the modelled enum is refused rather than silently matching nothing.
    let case = "unmodelled source";
    list_providers(&h, &json!({ "SubscriptionProviderSources": ["SUSE"] }))
        .await
        .error(case, 400, "ValidationException");
}

#[tokio::test]
async fn deregistration_is_observable_by_every_later_caller() {
    let h = harness();
    let mut arns = Vec::new();
    for secret_arn in SECRET_ARNS {
        let case = format!("register {secret_arn}");
        arns.push(provider_arn(
            &register(
                &h,
                &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
            )
            .await,
            &case,
        ));
    }

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("deregister {arn}");
        let reply = deregister(&h, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            reply.status, reply.body
        );

        // Gone for every later caller: Get refuses it and the list no longer reports it.
        get_provider(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
        let rows = listed(&list_providers(&h, &json!({})).await, &case);
        assert_eq!(
            rows.iter().map(|(arn, _, _, _)| arn.clone()).collect::<Vec<_>>(),
            arns[index + 1..],
            "case {case:?}: the list must report only the registrations still in state"
        );

        // And a second deregistration of the same ARN has nothing to remove.
        deregister(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
    }
}

#[tokio::test]
async fn an_arn_no_registration_holds_is_refused() {
    let h = harness();
    // Well-formed ARNs (they satisfy the modelled pattern) that name nothing in state.
    let unknown = [
        "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider/00000000-0000-4000-8000-000000000009",
        "arn:aws:license-manager-linux-subscriptions:eu-central-1:123456789012:subscription-provider/other",
        "arn:aws:iam:us-west-2:000000000000:role/not-a-provider",
    ];
    for arn in unknown {
        let case = format!("unknown {arn}");
        get_provider(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
        deregister(&h, arn)
            .await
            .error(&case, 400, "ResourceNotFoundException");
    }

    // An ARN outside the modelled pattern is refused as invalid input instead.
    for arn in ["not-an-arn", "arn:aws:license-manager-linux-subscriptions:us-west-2:000000000000:subscription-provider"] {
        let case = format!("malformed {arn}");
        get_provider(&h, arn)
            .await
            .error(&case, 400, "ValidationException");
    }
}

#[tokio::test]
async fn the_provider_store_refuses_growth_past_its_cap() {
    let h = harness();
    // Fill the store to its ceiling directly, so the test exercises the boundary rather
    // than the thousands of requests it would take to reach it over the wire.
    {
        let state = h
            .engine
            .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>();
        let mut state = state.lock();
        for index in 0..MAX_PROVIDERS {
            let arn = format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/\
                 00000000-0000-4000-8000-{index:012x}"
            );
            let secret_arn = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:s{index}");
            state.providers.insert(
                arn.clone(),
                SubscriptionProviderRecord {
                    arn: arn.clone(),
                    source: RED_HAT.to_string(),
                    secret_arn: secret_arn.clone(),
                    status: "ACTIVE".to_string(),
                    status_message: None,
                    last_successful_data_retrieval_time: None,
                    tags: std::collections::HashMap::new(),
                },
            );
            state.provider_order.push(arn.clone());
            state
                .provider_index
                .insert((RED_HAT.to_string(), secret_arn), arn);
        }
    }

    // A new provider would take the store past its maximum entry count, so it is refused.
    let case = format!("registration number {}", MAX_PROVIDERS + 1);
    register(
        &h,
        &json!({
            "SubscriptionProviderSource": RED_HAT,
            "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:one-too-many",
        }),
    )
    .await
    .error(&case, 400, "ValidationException");

    // A re-registration adds no entry, so it is still served at the cap.
    let case = "re-registration at the cap";
    let existing = format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:s0");
    let reply = register(
        &h,
        &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": existing }),
    )
    .await;
    assert_eq!(
        provider_arn(&reply, case),
        format!(
            "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/\
             00000000-0000-4000-8000-000000000000"
        ),
        "case {case:?}: expected the ARN already in state"
    );
}

#[tokio::test]
async fn service_settings_round_trip_through_state() {
    let h = harness();

    // Before any update the account reports the default, never a canned "Enabled".
    let case = "settings before any update";
    let body = get_settings(&h).await.ok(case).clone();
    assert_eq!(
        body.get("LinuxSubscriptionsDiscovery").and_then(Value::as_str),
        Some("Disabled"),
        "case {case:?}: expected discovery Disabled by default, got {body:?}"
    );
    for absent in ["LinuxSubscriptionsDiscoverySettings", "HomeRegions"] {
        assert!(
            body.get(absent).is_none_or(Value::is_null),
            "case {case:?}: expected no {absent} before an update, got {body:?}"
        );
    }

    // A table over what the rule generalises across: the discovery switch, the
    // Organizations integration, and source-region lists of growing size (the modelled
    // `SourceRegions` is a unique list of 1..100 entries).
    let region_sets: &[&[&str]] = &[
        &["us-east-1"],
        &["us-west-2", "eu-west-1"],
        &["ap-southeast-1", "us-east-2", "eu-central-1"],
    ];
    for discovery in ON_OFF {
        for integration in ON_OFF {
            for regions in region_sets {
                let case = format!("settings {discovery}/{integration}/{regions:?}");
                let request = json!({
                    "LinuxSubscriptionsDiscovery": discovery,
                    "LinuxSubscriptionsDiscoverySettings": {
                        "SourceRegions": regions,
                        "OrganizationIntegration": integration,
                    },
                });
                let updated = update_settings(&h, &request).await.ok(&case).clone();
                // The update answers with what it persisted, and a later read agrees.
                let fetched = get_settings(&h).await.ok(&case).clone();
                for body in [&updated, &fetched] {
                    assert_eq!(
                        body.get("LinuxSubscriptionsDiscovery").and_then(Value::as_str),
                        Some(*discovery),
                        "case {case:?}: expected discovery {discovery}, got {body:?}"
                    );
                    assert_eq!(
                        body.get("Status").and_then(Value::as_str),
                        Some("Successful"),
                        "case {case:?}: expected Status Successful, got {body:?}"
                    );
                    assert_eq!(
                        body.get("HomeRegions").and_then(Value::as_array),
                        Some(&vec![json!(REGION)]),
                        "case {case:?}: expected the signed Region as the home Region, got {body:?}"
                    );
                    let settings = body
                        .get("LinuxSubscriptionsDiscoverySettings")
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: no LinuxSubscriptionsDiscoverySettings in {body:?}")
                        });
                    assert_eq!(
                        settings.get("OrganizationIntegration").and_then(Value::as_str),
                        Some(*integration),
                        "case {case:?}: expected OrganizationIntegration {integration}, got {settings:?}"
                    );
                    assert_eq!(
                        settings.get("SourceRegions").and_then(Value::as_array),
                        Some(&regions.iter().map(|r| json!(r)).collect::<Vec<_>>()),
                        "case {case:?}: expected the persisted SourceRegions, got {settings:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_second_home_region_needs_allow_update() {
    let h = harness();
    let settings = json!({
        "LinuxSubscriptionsDiscovery": "Enabled",
        "LinuxSubscriptionsDiscoverySettings": {
            "SourceRegions": ["us-east-1"],
            "OrganizationIntegration": "Disabled",
        },
    });

    let case = "first update, in us-west-2";
    update_settings(&h, &settings).await.ok(case);

    // Updating again from the SAME home Region adds no aggregation Region, so it needs
    // no AllowUpdate.
    let case = "repeat update in the home Region";
    let body = update_settings(&h, &settings).await.ok(case).clone();
    assert_eq!(
        body.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION)]),
        "case {case:?}: expected the home Region unchanged, got {body:?}"
    );

    // From another Region it would aggregate the data in a second home Region, which is
    // what AllowUpdate admits.
    let other = "eu-west-1";
    let case = "update from another Region without AllowUpdate";
    call_in_region(
        &h,
        other,
        "/subscription/UpdateServiceSettings",
        &settings,
    )
    .await
    .error(case, 400, "ValidationException");

    let case = "update from another Region with AllowUpdate";
    let mut allowed = settings.clone();
    allowed["AllowUpdate"] = json!(true);
    let body = call_in_region(&h, other, "/subscription/UpdateServiceSettings", &allowed)
        .await
        .ok(case)
        .clone();
    assert_eq!(
        body.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION), json!(other)]),
        "case {case:?}: expected both home Regions, got {body:?}"
    );
    // And the second home Region is persisted, not just echoed.
    let fetched = get_settings(&h).await.ok(case).clone();
    assert_eq!(
        fetched.get("HomeRegions").and_then(Value::as_array),
        Some(&vec![json!(REGION), json!(other)]),
        "case {case:?}: expected both home Regions from state, got {fetched:?}"
    );
}
