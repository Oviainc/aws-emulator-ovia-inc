// SPDX-License-Identifier: CC0-1.0

//! The shared wire harness for this crate's suites: one emulator, one request builder,
//! and one reply assertion vocabulary, so no suite grows its own.
//!
//! Every call goes through the router's `dispatch`, so a request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be — which is
//! the only way a suite can observe a refusal the framework states BEFORE a handler runs
//! (this service rewords those in `src/service/validation.rs`).
//!
//! A suite whose rule is about the CREDENTIAL a registration names takes its emulator
//! from [`harness_with_secrets_manager`], which mounts Secrets Manager beside this service
//! on the same engine — the way the assembled emulator runs them — because
//! `RegisterSubscriptionProvider` reads the secret its `SecretArn` names by calling it.
//!
//! `allow(dead_code)`: this module is the vocabulary for every suite, so an individual
//! suite uses only the part of it that its rules need.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    DiscoveredInstanceRecord, SubscriptionProviderRecord, PROVIDER_STATUS_ACTIVE,
    PROVIDER_STATUS_INVALID, PROVIDER_STATUS_PENDING, TOKEN_VALIDATION_DELAY,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::{
    ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState,
    EmulatedService,
};

/// This service's sigv4 signing name (`OPERATIONS.md`, "Wire protocol").
pub const SIGNING_NAME: &str = "license-manager-linux-subscriptions";
/// The region the test requests are signed for.
pub const REGION: &str = "us-west-2";
/// The canonical account id every emulated ARN carries.
pub const ACCOUNT: &str = "000000000000";
/// The only `SubscriptionProviderSource` the model declares (`one of: RedHat`).
pub const RED_HAT: &str = "RedHat";
/// `SubscriptionProviderStatus` of a provider whose token the service holds and
/// accepted — the status a registration reaches once it is usable — named from the crate
/// so a suite cannot hold a second copy of the value.
pub const ACTIVE: &str = PROVIDER_STATUS_ACTIVE;
/// `SubscriptionProviderStatus` a fresh registration reports, while the validation of its
/// access token has not landed.
pub const PENDING: &str = PROVIDER_STATUS_PENDING;
/// `SubscriptionProviderStatus` of a registration whose access token the service rejected.
pub const INVALID: &str = PROVIDER_STATUS_INVALID;

/// The message `RegisterSubscriptionProvider` refuses a credential it cannot use with:
/// recorded behaviour, held here as the literal a suite compares against so no suite reads
/// it back out of the handler it is testing.
pub const TOKEN_REFUSAL_MESSAGE: &str = "The subscription provider token is invalid or expired";

/// Base64url-encode `bytes` the way a JWT segment is encoded; `pad` selects whether the
/// segment carries the `=` padding, which a token may be encoded with or without.
///
/// The codec is `aws_smithy_types::base64` — the standard-alphabet one this crate already
/// uses — plus the two alphabet substitutions (`+` -> `-`, `/` -> `_`) that turn RFC 4648
/// §4 into the §5 base64url a compact JWS carries, which is the same translation the
/// handler side reverses in `service_common::jwt`.
pub fn base64url(bytes: &[u8], pad: bool) -> String {
    let standard = aws_smithy_types::base64::encode(bytes);
    let encoded = standard.replace('+', "-").replace('/', "_");
    if pad {
        encoded
    } else {
        encoded.trim_end_matches('=').to_string()
    }
}

/// The signature segment a test token carries. A JWT's third segment is the signature over
/// the first two; nothing in this emulator can verify one (that needs the provider's keys),
/// so a case only has to carry a segment that is there.
pub const SIGNATURE_SEGMENT: &str = "c2lnbmF0dXJl";

/// A JSON Web Token carrying `header` and `payload`, encoded as the three `.`-separated
/// base64url segments a token issued by an OIDC single sign-on is.
pub fn jwt(header: &Value, payload: &Value, pad: bool) -> String {
    let segment = |value: &Value| {
        base64url(
            &serde_json::to_vec(value).expect("a test token's JSON serializes"),
            pad,
        )
    };
    format!(
        "{}.{}.{SIGNATURE_SEGMENT}",
        segment(header),
        segment(payload)
    )
}

/// The header of a well-formed token: a JOSE header names the signing algorithm
/// (RFC 7515 §4.1.1). Any algorithm will do for a test token — what a service reading one
/// checks is that the header names one, not which.
pub fn token_header() -> Value {
    json!({ "alg": "RS256", "typ": "JWT" })
}

/// Wait out the span the clock owes a fresh registration before its terminal
/// `SubscriptionProviderStatus` is due, so the next call materializes it.
///
/// The emulator never sleeps for the transition — it stores the due instant and
/// materializes it on the next call that reports a status — so it is the TEST that has to
/// let the clock pass the span. `std::thread::sleep` rather than `tokio::time::sleep`
/// because this workspace enables tokio's `macros`/`rt` features only, and nothing else is
/// pending on the test's runtime while it waits.
pub fn wait_for_token_validation() {
    std::thread::sleep(TOKEN_VALIDATION_DELAY + std::time::Duration::from_millis(100));
}

/// One emulator: the router that dispatches, plus the shared engine its service was
/// built from (so a test can read or pre-fill the store the handlers use).
pub struct Harness {
    pub router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

pub fn harness() -> Harness {
    build_harness(false)
}

/// One emulator with Secrets Manager mounted BESIDE this service on the same engine,
/// exactly as the assembled emulator runs them, because `RegisterSubscriptionProvider`
/// reads the secret its `SecretArn` names by calling Secrets Manager in process.
pub fn harness_with_secrets_manager() -> Harness {
    build_harness(true)
}

fn build_harness(with_secrets_manager: bool) -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests sign nothing; authentication is the framework's concern, not this
    // service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    if with_secrets_manager {
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
    }
    Harness { router, engine }
}

/// Store a secret through the MOUNTED Secrets Manager service and answer the ARN it
/// minted, so a `SecretArn` a case means to be real is one Secrets Manager can read.
///
/// `members` are the `CreateSecret` members past `Name`, so a case can store the secret's
/// value as a `SecretString` or as a `SecretBinary`. Secrets Manager is `awsJson1_1`
/// (`POST /` with `X-Amz-Target: secretsmanager.<Operation>`), which is a different wire
/// protocol from this service's `restJson1` — hence its own request builder here.
pub async fn create_secret(h: &Harness, name: &str, members: &Value) -> String {
    let mut body = json!({ "Name": name });
    for (member, value) in members
        .as_object()
        .expect("the CreateSecret members are a JSON object")
    {
        body[member] = value.clone();
    }
    // A create that stores a value creates that version, and Secrets Manager
    // requires the caller to name it: "If you generate a raw HTTP request to the
    // Secrets Manager service endpoint, then you must generate a
    // ClientRequestToken and include it in the request."
    // (`CreateSecretRequest$ClientRequestToken`, that service's Smithy model.)
    if body.get("SecretString").is_some() || body.get("SecretBinary").is_some() {
        let token: String = format!("client-request-token-{name}")
            .chars()
            .chain(std::iter::repeat('-'))
            .take(40)
            .collect();
        body["ClientRequestToken"] = json!(token);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://secretsmanager.{REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "secretsmanager.CreateSecret")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{REGION}/secretsmanager\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the CreateSecret body serializes"),
        ))
        .expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let reply: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "CreateSecret must succeed so a case has a real secret, got {status} with {reply:?}"
    );
    reply["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSecret reports the new secret's ARN, got {reply:?}"))
        .to_string()
}

/// What a call answered: the HTTP status, the wire error code (if any) and the body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub error_type: Option<String>,
    pub body: Value,
}

impl Reply {
    pub fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert this reply is the client error `code` with HTTP `status`.
    pub fn error(&self, case: &str, status: u16, code: &str) {
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

    /// The `message` member of an error body, or `""` when there is none.
    pub fn message(&self) -> &str {
        self.body.get("message").and_then(Value::as_str).unwrap_or("")
    }
}

/// Dispatch one request: `method` and `path` (which may carry a query string) exactly as
/// a client would send them, signed for `region`, with `body` as the JSON payload.
pub async fn send(
    h: &Harness,
    method: &str,
    region: &str,
    path: &str,
    body: Option<&Value>,
) -> Reply {
    let payload = match body {
        Some(body) => serde_json::to_vec(body).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{SIGNING_NAME}.{region}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{region}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
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

pub async fn call_in_region(h: &Harness, region: &str, path: &str, body: &Value) -> Reply {
    send(h, "POST", region, path, Some(body)).await
}

pub async fn call(h: &Harness, path: &str, body: &Value) -> Reply {
    call_in_region(h, REGION, path, body).await
}

pub async fn register(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/RegisterSubscriptionProvider", body).await
}

pub async fn get_provider(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/GetRegisteredSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

pub async fn list_providers(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/ListRegisteredSubscriptionProviders", body).await
}

pub async fn deregister(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/DeregisterSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

pub async fn get_settings(h: &Harness) -> Reply {
    call(h, "/subscription/GetServiceSettings", &json!({})).await
}

pub async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/UpdateServiceSettings", body).await
}

/// Persist this account's settings with `LinuxSubscriptionsDiscovery` ENABLED for
/// `source_regions`, over the wire, the way a client onboards the account.
///
/// A suite that places discovered records itself needs the settings that make them
/// discovered data: what the two lists report is the inventory of the Regions the account's
/// settings currently discover, so records held for a Region those settings do not name are
/// a state no discovery pass could have left behind.
pub async fn enable_discovery(h: &Harness, source_regions: &[&str]) {
    let case = &format!("UpdateServiceSettings enabling discovery for {source_regions:?}");
    let reply = update_settings(
        h,
        &json!({
            "LinuxSubscriptionsDiscovery": "Enabled",
            "LinuxSubscriptionsDiscoverySettings": {
                "OrganizationIntegration": "Disabled",
                "SourceRegions": source_regions,
            },
            "AllowUpdate": true,
        }),
    )
    .await;
    reply.ok(case);
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}`.
pub async fn list_tags(h: &Harness, resource_arn: &str) -> Reply {
    send(
        h,
        "GET",
        REGION,
        &format!("/tags/{}", percent_encode(resource_arn)),
        None,
    )
    .await
}

/// The tag set a `ListTagsForResource` body reports, as a sorted map so two sets compare
/// independently of iteration order.
///
/// The ONE projection of that reply in this crate: a suite asserting a tag set states the
/// set it expects and reads the stored one through here, so the same malformed reply is
/// reported the same way whichever suite observed it.
pub fn reported_tags(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    let body = reply.ok(case);
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags map in {body:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// The tag set the resource `arn` carries, read back over the wire through [`list_tags`]
/// and projected by [`reported_tags`] — the form a suite holding an ARN rather than a
/// reply uses.
pub async fn reported_tags_of(h: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    reported_tags(&list_tags(h, arn).await, case)
}

/// `TagResource`: `PUT /tags/{resourceArn}`, whose body carries the `tags` map.
pub async fn tag(h: &Harness, resource_arn: &str, tags: &Value) -> Reply {
    send(
        h,
        "PUT",
        REGION,
        &format!("/tags/{}", percent_encode(resource_arn)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// `UntagResource`: `DELETE /tags/{resourceArn}?tagKeys=…`.
pub async fn untag(h: &Harness, resource_arn: &str, tag_keys: &[&str]) -> Reply {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect();
    let path = format!(
        "/tags/{}?{}",
        percent_encode(resource_arn),
        query.join("&")
    );
    send(h, "DELETE", REGION, &path, None).await
}

/// Percent-encode everything outside the unreserved set (RFC 3986 §2.3:
/// `ALPHA / DIGIT / "-" / "." / "_" / "~"`), so a label or a query value carrying `:`,
/// `/` or a space reaches the server as the one value it is.
///
/// Written out here rather than pulled from an encoder crate ON PURPOSE: this is a
/// three-line client-side helper used only by this crate's tests, and adding a
/// `[dev-dependencies]` entry for it changes `Cargo.lock` — the one file every other
/// change in this repository also touches, which makes a fix that needs none of it
/// conflict on landing. The server side's decoding is what the tests actually exercise.
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Whether `value` is an ISO-8601 UTC instant at whole-second precision
/// (`YYYY-MM-DDTHH:MM:SSZ`), the form this service renders a timestamp member in.
///
/// Checked by the suites rather than taken on trust, because the members carrying an
/// instant here are plain `String`s in the model: the format is part of what a client
/// parses.
pub fn is_iso8601_utc(value: &str) -> bool {
    let digits =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_digit());
    let Some((date, time)) = value.split_once('T') else {
        return false;
    };
    let Some(time) = time.strip_suffix('Z') else {
        return false;
    };
    let date: Vec<&str> = date.split('-').collect();
    let time: Vec<&str> = time.split(':').collect();
    match (date.as_slice(), time.as_slice()) {
        ([year, month, day], [hour, minute, second]) => {
            digits(year, 4)
                && digits(month, 2)
                && digits(day, 2)
                && digits(hour, 2)
                && digits(minute, 2)
                && digits(second, 2)
        }
        _ => false,
    }
}

/// An ARN of this service's one resource kind, for `id`.
pub fn provider_arn_for(id: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/{id}")
}

/// Seed one registration DIRECTLY into the store, in a TERMINAL `ACTIVE` status, and
/// answer its ARN.
///
/// `RegisterSubscriptionProvider` creates registrations over the wire and they become
/// `ACTIVE` on the clock (which is what
/// `tests/all/subscription_provider_registration.rs` exercises). A suite whose rule is
/// about the read/list/tag/deregister side of an ALREADY usable registration places the
/// record itself instead, through the same seam the handlers use (`engine.state::<…>()`,
/// not a second copy of the store), so it does not have to wait out that span once per
/// row.
pub fn seed_provider(
    h: &Harness,
    id: &str,
    secret_arn: &str,
    tags: &[(&str, &str)],
) -> String {
    let arn = provider_arn_for(id);
    let state = h
        .engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>();
    let mut state = state.lock();
    state.providers.insert(
        arn.clone(),
        SubscriptionProviderRecord::settled(
            arn.clone(),
            RED_HAT.to_string(),
            secret_arn.to_string(),
            ACTIVE.to_string(),
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<HashMap<String, String>>(),
            std::time::SystemTime::now(),
        ),
    );
    arn
}

/// Put one discovered instance DIRECTLY into the store, keyed by its `instance_id`, and
/// answer that id.
///
/// Discovery reads the account's EC2 inventory in the configured source Regions, and no
/// operation of THIS service writes a discovered instance, so a suite whose rule is about
/// what the two discovery lists project places the records itself — through the same
/// `engine.state::<…>()` seam the handlers use, not a second copy of the store.
pub fn seed_discovered_instance(h: &Harness, record: DiscoveredInstanceRecord) -> String {
    let instance_id = record.instance_id.clone();
    let state = h
        .engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>();
    state
        .lock()
        .discovered_instances
        .insert(instance_id.clone(), record);
    instance_id
}

/// How many registrations the store holds.
pub fn stored_provider_count(h: &Harness) -> usize {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .providers
        .len()
}

/// The `SubscriptionProviderArn` member of a 200 body.
pub fn provider_arn(reply: &Reply, case: &str) -> String {
    reply
        .ok(case)
        .get("SubscriptionProviderArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionProviderArn in {:?}", reply.body))
        .to_string()
}

/// A string member of a 200 body, or `None` when the body does not carry it.
pub fn member<'a>(reply: &'a Reply, case: &str, name: &str) -> Option<&'a str> {
    reply.ok(case).get(name).and_then(Value::as_str)
}

/// The providers a list body reports, as `(arn, source, secret arn, status)` rows.
pub fn listed(reply: &Reply, case: &str) -> Vec<(String, String, String, String)> {
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
