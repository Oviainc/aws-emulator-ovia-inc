// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the backup gateway lifecycle: `CreateGateway`, `GetGateway`,
//! `ListGateways`, `UpdateGatewayInformation`, `DeleteGateway`, `TagResource` and
//! `ListTagsForResource`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter` — the same path a socket-served request takes — so what is
//! asserted is the wire behaviour, and the only way a later request can see what an
//! earlier one wrote is the shared store.
//!
//! The rule under test is the same for every operation here: each one resolves the
//! gateway its ARN names out of the persisted store and answers from that record, so a
//! create is visible to every later read, an update replaces only the members the
//! request carried, and a delete makes the record absent. The tables below span that
//! class — several gateways differing in display name and tag count, and
//! every page size over a fixed set of records — rather than one recorded request.
//!
//! The request harness here ([`call`], [`build_router`], [`Reply`]) is shared with the
//! sibling suites in this directory, which reach it as `crate::gateway_lifecycle::*`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    service_state, Creation, BACKUP_VM, DEFAULT_PAGE_SIZE, GATEWAY_SOFTWARE_VERSION, MAX_PAGE_SIZE,
    MAX_STORED_GATEWAYS,
};

/// The `X-Amz-Target` prefix for this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "BackupOnPremises_v20210101";

/// The region the requests below are signed for. Deliberately NOT the emulator's
/// `us-east-1` fallback: a minted ARN has to carry the region the caller addressed.
pub(crate) const REGION: &str = "us-west-2";

/// The tolerance a reported instant is compared with: the wire carries the modelled `Time`
/// as epoch seconds, so a millisecond of rounding is not a difference.
pub(crate) const INSTANT_TOLERANCE_SECS: f64 = 0.002;

/// `time` as epoch seconds, the form the modelled `Time` timestamp takes on the wire —
/// shared with the sibling suites, so one reported instant is read one way everywhere.
pub(crate) fn epoch_secs(time: SystemTime) -> f64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// The activation key the helpers below create gateways with: one the service ISSUED.
///
/// A create REDEEMS the activation its key names, and a key is minted only by the
/// activation handshake a deployed appliance makes — so this harness makes that handshake
/// over the wire, exactly as a client would, rather than seeding the registry in process.
/// Each call issues a new key, because a redeemed activation is spent.
pub(crate) async fn issued_activation_key(router: &MultiServiceRouter) -> String {
    issued_activation_key_for(router, REGION).await
}

/// [`issued_activation_key`], for a named region: a key is issued for ONE region, which is
/// what the handshake's `activationRegion` names.
pub(crate) async fn issued_activation_key_for(
    router: &MultiServiceRouter,
    region: &str,
) -> String {
    let reply = activation_handshake(router, &format!("activationRegion={region}")).await;
    assert_eq!(
        reply.0, 200,
        "the activation handshake for {region:?} must issue a key, got {} {:?}",
        reply.0, reply.1
    );
    reply.1
}

/// One activation handshake: the unsigned `GET /?<query>` a deployed appliance makes, and
/// the status and body it is answered with.
pub(crate) async fn activation_handshake(
    router: &MultiServiceRouter,
    query: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/?{query}"))
        .header("host", "backup-gateway.us-west-2.amazonaws.com")
        .body(Bytes::new())
        .expect("build the activation request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec())
        .expect("the activation handshake answers in text");
    (status, body)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`backup-gateway`) and `region`. The signature itself is never checked here:
/// the test router runs with request authentication off, so what this carries is the
/// REGION the request was addressed to, which is what a minted ARN and a redeemed
/// activation are read off.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/backup-gateway/aws4_request, \
         SignedHeaders=host, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// What one dispatched request answered.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The awsJson error discriminator, namespace stripped.
    pub(crate) code: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn str_at(&self, pointer: &str) -> Option<&str> {
        self.body.pointer(pointer).and_then(Value::as_str)
    }

    pub(crate) fn gateway_arn(&self) -> &str {
        self.str_at("/GatewayArn")
            .unwrap_or_else(|| panic!("no GatewayArn in {:?}", self.body))
    }

    /// The returned `Tags` list as a map of the CLIENT's own tags — the pairs a
    /// `TagResource` (or a create) wrote.
    ///
    /// A map, not a list, so a test compares tag SETS and not the order a list happens to
    /// carry; and the AWS-assigned system tags every resource a request creates also
    /// carries (`aws:createdBy`, `aws:createdDate` —
    /// `backup_gateway::state::is_reserved_tag_key`) are left out, because no client wrote
    /// them and no client can write or remove one. Their presence, their keys and their
    /// POSITION in the reported list are a rule of their own, pinned by the
    /// `aws_assigned_system_tags` suite through [`Reply::reported_tags`].
    pub(crate) fn tags(&self) -> BTreeMap<String, String> {
        self.reported_tags()
            .into_iter()
            .filter(|(key, _)| !backup_gateway::state::is_reserved_tag_key(key))
            .collect()
    }

    /// The returned `Tags` list exactly as the reply carried it, in order: every pair,
    /// AWS-assigned and client-written alike.
    pub(crate) fn reported_tags(&self) -> Vec<(String, String)> {
        self.body
            .get("Tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn gateway_arns(&self) -> Vec<String> {
        self.body
            .get("Gateways")
            .and_then(Value::as_array)
            .map(|gateways| {
                gateways
                    .iter()
                    .map(|gateway| {
                        gateway["GatewayArn"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn next_token(&self) -> Option<String> {
        self.str_at("/NextToken").map(str::to_string)
    }
}

pub(crate) fn build_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// How many gateways the store holds — the one place the sibling suites read it from, so
/// "the store did not grow" means the same thing in all of them.
pub(crate) fn gateway_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .gateway_count()
}

/// How many activation keys the store holds pending redemption — read straight out of the
/// store, so a test can tell an activation that was really issued (or really spent) from a
/// response that merely looks right.
pub(crate) fn pending_activation_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .pending_activation_count()
}

/// How many hypervisors the store holds — the one place the sibling suites read it from,
/// for the same reason as [`gateway_count`].
pub(crate) fn hypervisor_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisor_count()
}

/// The CLIENT tags `arn` carries according to the STORE, not the wire: what a response
/// reports has to be the record that is really persisted.
///
/// The AWS-assigned system tags are left out for the same reason [`Reply::tags`] leaves
/// them out of the wire's answer — no client wrote them — so the two are compared in one
/// vocabulary.
pub(crate) fn persisted_client_tags(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> BTreeMap<String, String> {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .resource_tags(arn, SystemTime::now())
        .map(|(_, tags)| {
            tags.into_iter()
                .filter(|tag| !backup_gateway::state::is_reserved_tag_key(&tag.key))
                .map(|tag| (tag.key, tag.value))
                .collect::<BTreeMap<String, String>>()
        })
        .unwrap_or_else(|| panic!("no resource stored under {arn}"))
}

/// The ARNs the STORE holds a hypervisor record for `host` under, read out of the records
/// rather than off the wire.
///
/// A set, because `Host` is not a key: any number of records may carry one host, so every
/// suite that asks this question asks it of all of them.
pub(crate) fn hypervisor_arns_for_host(
    engine: &Arc<engine::InMemoryEngine>,
    host: &str,
) -> BTreeSet<String> {
    service_state(&**engine)
        .expect("the in-memory engine must expose this state")
        .lock()
        .hypervisors_for_host(host)
        .iter()
        .map(|record| record.arn.clone())
        .collect()
}

/// The router every test dispatches through.
pub(crate) fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(backup_gateway::EmulatedService::new(Arc::clone(engine)));
    router
}

pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Reply {
    call_in_region(router, REGION, operation, body).await
}

/// [`call`], addressed to a named region: the region a request is signed for is the one a
/// minted ARN carries and the one an activation has to have been issued for.
pub(crate) async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "backup-gateway.us-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    // The awsJson families carry the error shape in the body's `__type`; the boundary
    // refusal path also sets the header. Either way the wire code is the part after the
    // namespace.
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(header_code)
        .map(|raw| raw.rsplit('#').next().unwrap_or(&raw).to_string());
    Reply {
        status,
        code,
        body: parsed,
    }
}

pub(crate) fn create_body(
    display_name: &str,
    activation_key: &str,
    tags: &BTreeMap<String, String>,
) -> Value {
    let mut body = json!({
        "ActivationKey": activation_key,
        "GatewayDisplayName": display_name,
        "GatewayType": BACKUP_VM,
    });
    if !tags.is_empty() {
        body["Tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

fn tag_map<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The modelled `GatewayArn` pattern, spelled out here rather than reused from the
/// crate, so the assertion is independent of the code that mints the ARN:
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// with `@length(min: 50, max: 180)`
/// (`smithy_query(query="shape",shape="GatewayArn")`).
fn matches_modelled_gateway_arn(arn: &str) -> bool {
    if arn.len() < 50 || arn.len() > 180 {
        return false;
    }
    let segment = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some((resource_type, id)) = resource.split_once('/') else {
        return false;
    };
    *literal == "arn"
        && matches!(*partition, "aws" | "aws-cn" | "aws-us-gov")
        && *service == "backup-gateway"
        && [*region, *account, resource_type, id]
            .into_iter()
            .all(segment)
}

/// A well-formed `GatewayArn` for a gateway that was never created — the shape the
/// modelled pattern accepts, so it reaches the handler rather than the boundary
/// validator.
pub(crate) fn never_created_arn() -> String {
    format!(
        "arn:aws:backup-gateway:{REGION}:{}:gateway/bgw-0000000000000999",
        engine::EMULATOR_ACCOUNT_ID
    )
}

pub(crate) async fn create(router: &MultiServiceRouter, display_name: &str) -> String {
    let activation_key = issued_activation_key(router).await;
    let reply = call(
        router,
        "CreateGateway",
        &create_body(display_name, &activation_key, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {display_name:?}: CreateGateway must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.gateway_arn().to_string()
}

/// Walk every page of `ListGateways` at one `MaxResults`, asserting the page-level
/// invariants as it goes, and return the ARNs in the order they were handed out.
async fn walk_pages(router: &MultiServiceRouter, max_results: Option<i64>) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = call(router, "ListGateways", &body).await;
        assert_eq!(
            reply.status, 200,
            "case MaxResults={max_results:?} page {pages}: ListGateways must answer 200, \
             got {} {:?}",
            reply.status, reply.body
        );
        let page = reply.gateway_arns();
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case MaxResults={max_results:?} page {pages}: a page must hold at most \
                 {max} gateways, got {}",
                page.len()
            );
        }
        token = reply.next_token();
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case MaxResults={max_results:?} page {pages}: a page that promises more \
                 records must not be empty, got {:?}",
                reply.body
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages < 64,
            "case MaxResults={max_results:?}: pagination did not terminate after {pages} \
             pages, seen {seen:?}"
        );
        if token.is_none() {
            return seen;
        }
    }
}

#[tokio::test]
async fn gateway_lifecycle_round_trips_through_the_store() {
    let engine = build_engine();
    let router = build_router(&engine);

    let created = call(
        &router,
        "CreateGateway",
        &create_body(
            "my-gateway",
            &issued_activation_key(&router).await,
            &tag_map([("env", "test")]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200, got {} {:?}",
        created.status, created.body
    );
    let arn = created.gateway_arn().to_string();
    assert!(
        matches_modelled_gateway_arn(&arn),
        "case create: the returned ARN must satisfy the modelled GatewayArn pattern and \
         length, got {arn:?}"
    );
    assert_eq!(
        arn,
        format!(
            "arn:aws:backup-gateway:{REGION}:{}:gateway/{}",
            engine::EMULATOR_ACCOUNT_ID,
            arn.rsplit('/').next().unwrap_or_default()
        ),
        "case create: the ARN must carry the signed-for region and the emulator account, \
         got {arn:?}"
    );

    // The record is really in the store, under the ARN the response returned.
    let stored_display_name = service_state(&*engine)
        .expect("the in-memory engine must expose this service's state")
        .lock()
        .gateway(&arn)
        .map(|gateway| gateway.display_name.clone());
    assert_eq!(
        stored_display_name.as_deref(),
        Some("my-gateway"),
        "case create: the gateway must be persisted under the ARN it returned, store held \
         {stored_display_name:?}"
    );

    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        fetched.status, 200,
        "case get: expected 200, got {} {:?}",
        fetched.status, fetched.body
    );
    for (member, expected) in [
        ("/Gateway/GatewayArn", arn.as_str()),
        ("/Gateway/GatewayDisplayName", "my-gateway"),
        ("/Gateway/GatewayType", BACKUP_VM),
        ("/Gateway/SoftwareVersion", GATEWAY_SOFTWARE_VERSION),
    ] {
        assert_eq!(
            fetched.str_at(member),
            Some(expected),
            "case get {member}: expected {expected:?}, got {:?}",
            fetched.body
        );
    }
    assert!(
        fetched
            .body
            .pointer("/Gateway/LastSeenTime")
            .and_then(Value::as_f64)
            .is_some(),
        "case get: LastSeenTime must be a timestamp, got {:?}",
        fetched.body
    );
    assert!(
        fetched.body.pointer("/Gateway/HypervisorId").is_none(),
        "case get /Gateway/HypervisorId: a gateway associated with no hypervisor must omit \
         it, got {:?}",
        fetched.body
    );
    // An activated gateway always has a maintenance window, so a gateway no
    // `PutMaintenanceStartTime` has touched reports the one its activation assigned
    // (see the `default_maintenance_window` suite for the rule over a table of creates).
    assert_eq!(
        fetched.body.pointer("/Gateway/MaintenanceStartTime"),
        Some(&crate::default_maintenance_window::default_window()),
        "case get /Gateway/MaintenanceStartTime: a gateway must report the window its \
         activation assigned, got {:?}",
        fetched.body
    );

    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        listed.gateway_arns(),
        vec![arn.clone()],
        "case list: the created gateway must be listed, got {:?}",
        listed.body
    );
    assert_eq!(
        listed.next_token(),
        None,
        "case list: a single final page must omit NextToken, got {:?}",
        listed.body
    );

    let tags = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        tags.tags(),
        tag_map([("env", "test")]),
        "case list-tags: the tags passed to CreateGateway must be persisted, got {:?}",
        tags.body
    );

    let tagged = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": [{ "Key": "owner", "Value": "qa" }] }),
    )
    .await;
    assert_eq!(
        (tagged.status, tagged.str_at("/ResourceARN")),
        (200, Some(arn.as_str())),
        "case tag: expected 200 echoing the resource ARN, got {} {:?}",
        tagged.status,
        tagged.body
    );
    let tags = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        tags.tags(),
        tag_map([("env", "test"), ("owner", "qa")]),
        "case tag: TagResource must MERGE into the stored tags, got {:?}",
        tags.body
    );

    let updated = call(
        &router,
        "UpdateGatewayInformation",
        &json!({ "GatewayArn": arn, "GatewayDisplayName": "my-gateway-2" }),
    )
    .await;
    assert_eq!(
        (updated.status, updated.str_at("/GatewayArn")),
        (200, Some(arn.as_str())),
        "case update: expected 200 echoing the gateway ARN, got {} {:?}",
        updated.status,
        updated.body
    );
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        fetched.str_at("/Gateway/GatewayDisplayName"),
        Some("my-gateway-2"),
        "case update: a later GetGateway must observe the new display name, got {:?}",
        fetched.body
    );
    let tags = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        tags.tags(),
        tag_map([("env", "test"), ("owner", "qa")]),
        "case update: updating the display name must leave the tags alone, got {:?}",
        tags.body
    );

    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        (deleted.status, deleted.str_at("/GatewayArn")),
        (200, Some(arn.as_str())),
        "case delete: expected 200 echoing the gateway ARN, got {} {:?}",
        deleted.status,
        deleted.body
    );

    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        (fetched.status, fetched.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "case get-after-delete: expected 400 ResourceNotFoundException, got {} {:?}",
        fetched.status,
        fetched.body
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert!(
        listed.gateway_arns().is_empty(),
        "case list-after-delete: the deleted gateway must be gone, got {:?}",
        listed.body
    );
    let deleted_again = call(&router, "DeleteGateway", &json!({ "GatewayArn": arn })).await;
    assert_eq!(
        (deleted_again.status, deleted_again.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "case delete-twice: a gateway that is no longer stored is not found, got {} {:?}",
        deleted_again.status,
        deleted_again.body
    );
}

/// Each create mints its OWN record: the rows differ along what the rule generalises
/// over — the display name and how many tags the request carries — and every row must come
/// back exactly as it was stored, under an ARN no other row shares.
///
/// Each row redeems its own activation, because an activation is spent by the create that
/// redeems it.
#[tokio::test]
async fn every_created_gateway_is_its_own_record() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, GatewayDisplayName, Tags)
    let rows: [(&str, &str, BTreeMap<String, String>); 5] = [
        ("no tags", "gw-plain", BTreeMap::new()),
        ("one tag", "gw-one-tag", tag_map([("env", "test")])),
        (
            "three tags",
            "gw-three-tags",
            tag_map([("env", "prod"), ("owner", "qa"), ("tier", "gold")]),
        ),
        (
            "single-character names",
            "g",
            tag_map([("k", "")]), // TagValue's modelled length minimum is 0
        ),
        (
            "longest names the model allows",
            // Name: len 1..100.
            "g1234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567",
            tag_map([("env", "test")]),
        ),
    ];

    let mut arns = BTreeSet::new();
    for (case, display_name, tags) in &rows {
        let activation_key = issued_activation_key(&router).await;
        let created = call(
            &router,
            "CreateGateway",
            &create_body(display_name, &activation_key, tags),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateGateway must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = created.gateway_arn().to_string();
        assert!(
            matches_modelled_gateway_arn(&arn),
            "case {case:?}: the returned ARN must satisfy the modelled GatewayArn \
             pattern and length, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: the minted ARN {arn:?} was already handed to another gateway"
        );

        let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            (
                fetched.status,
                fetched.str_at("/Gateway/GatewayDisplayName"),
                fetched.str_at("/Gateway/GatewayType"),
                fetched.str_at("/Gateway/GatewayArn"),
            ),
            (200, Some(*display_name), Some(BACKUP_VM), Some(arn.as_str())),
            "case {case:?}: GetGateway must return the stored record, got {} {:?}",
            fetched.status,
            fetched.body
        );

        let listed_tags = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            (listed_tags.status, listed_tags.tags()),
            (200, tags.clone()),
            "case {case:?}: ListTagsForResource must return exactly the created tags, \
             got {} {:?}",
            listed_tags.status,
            listed_tags.body
        );
    }

    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        listed.gateway_arns().into_iter().collect::<BTreeSet<_>>(),
        arns,
        "every created gateway must be listed, got {:?}",
        listed.body
    );
}

/// Paging is an invariant, so it is asserted over the WHOLE bounded space of page sizes
/// for a fixed set of records — every `MaxResults` from 1 to one past the record count,
/// plus "no MaxResults" — rather than for one sampled page size: at any page size the
/// token chain must enumerate every stored gateway exactly once, in key order, and only
/// the final page may omit `NextToken`.
#[tokio::test]
async fn pagination_enumerates_every_gateway_exactly_once_at_every_page_size() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut all = Vec::new();
    for index in 0..5 {
        all.push(create(&router, &format!("gw-page-{index}")).await);
    }
    all.sort();

    for max_results in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)] {
        let seen = walk_pages(&router, max_results).await;
        assert_eq!(
            seen, all,
            "case MaxResults={max_results:?}: paging must enumerate every stored gateway \
             exactly once in key order, got {seen:?}"
        );
    }

    // A record removed between pages drops out of the listing, and the ones that stayed
    // put are still enumerated exactly once.
    let removed = all.remove(2);
    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": removed })).await;
    assert_eq!(
        deleted.status, 200,
        "case delete-mid-list: expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    for max_results in [None, Some(1), Some(2), Some(4)] {
        let seen = walk_pages(&router, max_results).await;
        assert_eq!(
            seen, all,
            "case MaxResults={max_results:?} after delete: the deleted gateway must be \
             absent and the rest enumerated once, got {seen:?}"
        );
    }
}

/// Every operation that takes an ARN resolves it in the store, so a well-formed ARN with
/// no record behind it is `ResourceNotFoundException` (HTTP 400, the status this service
/// answers its client refusals with) — not a 200 with an empty body, and not a 501.
#[tokio::test]
async fn an_arn_with_no_record_is_resource_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);
    // One real gateway exists, so a refusal below means "this ARN", not "the store is
    // empty".
    let live = create(&router, "gw-live").await;
    let missing = never_created_arn();

    // (case, operation, request body)
    let rows: [(&str, &str, Value); 5] = [
        ("get", "GetGateway", json!({ "GatewayArn": missing })),
        (
            "update",
            "UpdateGatewayInformation",
            json!({ "GatewayArn": missing, "GatewayDisplayName": "renamed" }),
        ),
        ("delete", "DeleteGateway", json!({ "GatewayArn": missing })),
        (
            "tag",
            "TagResource",
            json!({ "ResourceARN": missing, "Tags": [{ "Key": "env", "Value": "test" }] }),
        ),
        (
            "list-tags",
            "ListTagsForResource",
            json!({ "ResourceArn": missing }),
        ),
    ];

    for (case, operation, body) in rows {
        let reply = call(&router, operation, &body).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode"),
                reply.str_at("/Message"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("GatewayNotFound"),
                Some("The specified gateway was not found."),
            ),
            "case {case:?}: {operation} on an ARN with no record must answer 400 \
             ResourceNotFoundException / GatewayNotFound, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // And the live gateway was not touched by any of those refusals.
    let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        (fetched.status, fetched.str_at("/Gateway/GatewayDisplayName")),
        (200, Some("gw-live")),
        "the existing gateway must be unchanged after the refusals, got {} {:?}",
        fetched.status,
        fetched.body
    );
}

/// A request outside a constraint is refused with HTTP 400 `ValidationException`,
/// whether the constraint is one the model declares (length, pattern, enum, range,
/// required) or one only its docs state (the reserved `aws:` tag-key prefix) — never
/// silently accepted, and never answered with a 501.
#[tokio::test]
async fn a_request_outside_a_constraint_is_refused_with_400() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-validation").await;

    // (case, operation, body) — each row violates exactly one constraint.
    let rows: [(&str, &str, Value); 12] = [
        (
            "GatewayType outside the enum",
            "CreateGateway",
            json!({ "ActivationKey": "KEY1", "GatewayDisplayName": "gw", "GatewayType": "BACKUP_PHYSICAL" }),
        ),
        (
            "GatewayDisplayName shorter than the minimum",
            "CreateGateway",
            json!({ "ActivationKey": "KEY1", "GatewayDisplayName": "", "GatewayType": BACKUP_VM }),
        ),
        (
            "GatewayDisplayName longer than the maximum",
            "CreateGateway",
            json!({ "ActivationKey": "KEY1", "GatewayDisplayName": "g".repeat(101), "GatewayType": BACKUP_VM }),
        ),
        (
            "GatewayDisplayName outside the pattern",
            "CreateGateway",
            json!({ "ActivationKey": "KEY1", "GatewayDisplayName": "my_gateway", "GatewayType": BACKUP_VM }),
        ),
        (
            "ActivationKey outside the pattern",
            "CreateGateway",
            json!({ "ActivationKey": "KEY!", "GatewayDisplayName": "gw", "GatewayType": BACKUP_VM }),
        ),
        (
            "ActivationKey longer than the maximum",
            "CreateGateway",
            json!({ "ActivationKey": "K".repeat(51), "GatewayDisplayName": "gw", "GatewayType": BACKUP_VM }),
        ),
        (
            "required ActivationKey absent",
            "CreateGateway",
            json!({ "GatewayDisplayName": "gw", "GatewayType": BACKUP_VM }),
        ),
        (
            "reserved aws: tag key on create",
            "CreateGateway",
            json!({
                "ActivationKey": "KEY1",
                "GatewayDisplayName": "gw",
                "GatewayType": BACKUP_VM,
                "Tags": [{ "Key": "aws:created-by", "Value": "me" }],
            }),
        ),
        (
            "reserved aws: tag key on tag",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "AWS:Owner", "Value": "me" }] }),
        ),
        (
            "MaxResults below the modelled minimum",
            "ListGateways",
            json!({ "MaxResults": 0 }),
        ),
        (
            "NextToken shorter than the minimum",
            "ListGateways",
            json!({ "NextToken": "" }),
        ),
        (
            "GatewayArn outside the modelled pattern",
            "GetGateway",
            json!({ "GatewayArn": "arn:aws:backup-gateway:us-west-2:000000000000:gateway" }),
        ),
    ];

    for (case, operation, body) in rows {
        let reply = call(&router, operation, &body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: {operation} must answer 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // None of the refusals created or changed anything.
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        listed.gateway_arns(),
        vec![arn.clone()],
        "a refused request must not create a gateway, got {:?}",
        listed.body
    );
    let tags = call(&router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert!(
        tags.tags().is_empty(),
        "a refused TagResource must not store a tag, got {:?}",
        tags.body
    );
}

/// A `NextToken` is resolved, not parsed: only a token a previous `ListGateways`
/// response handed out resumes a listing, and anything else is refused rather than
/// quietly restarting from the first page.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    for index in 0..3 {
        create(&router, &format!("gw-token-{index}")).await;
    }

    let first = call(&router, "ListGateways", &json!({ "MaxResults": 1 })).await;
    let issued = first
        .next_token()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken, got {:?}", first.body));
    let resumed = call(
        &router,
        "ListGateways",
        &json!({ "MaxResults": 1, "NextToken": issued }),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case issued token: resuming must answer 200, got {} {:?}",
        resumed.status, resumed.body
    );
    assert_ne!(
        resumed.gateway_arns(),
        first.gateway_arns(),
        "case issued token: the second page must not repeat the first, got {:?}",
        resumed.body
    );

    // (case, the token as the client sends it)
    let rows: [(&str, String); 4] = [
        ("not hex at all", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        (
            "hex of something that is not a gateway ARN",
            hex::encode("hello"),
        ),
        (
            "hex of an ARN from another service",
            hex::encode("arn:aws:backup:us-west-2:000000000000:vault/default"),
        ),
    ];
    for (case, token) in rows {
        let reply = call(
            &router,
            "ListGateways",
            &json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token this service did not issue must answer 400 \
             ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// A read reports; it does not change what a later caller sees. Repeating each reporting
/// operation must answer identically, and a read must not move `LastSeenTime`, mint a
/// record or drop one.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-readonly").await;
    call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": [{ "Key": "env", "Value": "test" }] }),
    )
    .await;

    // (case, operation, body)
    let rows: [(&str, &str, Value); 3] = [
        ("get", "GetGateway", json!({ "GatewayArn": arn })),
        ("list", "ListGateways", json!({ "MaxResults": 1 })),
        (
            "list-tags",
            "ListTagsForResource",
            json!({ "ResourceArn": arn }),
        ),
    ];

    for (case, operation, body) in rows {
        let first = call(&router, operation, &body).await;
        let second = call(&router, operation, &body).await;
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {case:?}: repeating {operation} must answer identically, got {:?} then \
             {:?}",
            first.body,
            second.body
        );
    }

    let state = service_state(&*engine).expect("the in-memory engine must expose this state");
    let guard = state.lock();
    let stored = guard
        .gateway(&arn)
        .unwrap_or_else(|| panic!("the gateway must still be stored after the reads"));
    assert_eq!(
        (guard.gateway_count(), stored.display_name.as_str()),
        (1, "gw-readonly"),
        "the reads must leave exactly the one unchanged gateway in the store, got {} \
         records and {:?}",
        guard.gateway_count(),
        stored.display_name
    );
    assert_eq!(
        stored.last_seen_time, stored.created_time,
        "a read must not move LastSeenTime, got {:?} against a created time of {:?}",
        stored.last_seen_time, stored.created_time
    );
}

/// The store and the response are both bounded server-side: `CreateGateway` is the one
/// operation that grows resident memory, so a create past
/// [`MAX_STORED_GATEWAYS`](backup_gateway::state::MAX_STORED_GATEWAYS) is refused rather
/// than served, and a `ListGateways` that names no `MaxResults` is truncated at
/// [`DEFAULT_PAGE_SIZE`](backup_gateway::state::DEFAULT_PAGE_SIZE) with a `NextToken`
/// instead of serialising every record.
#[tokio::test]
async fn the_store_and_the_page_are_bounded() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Fill the store to its ceiling through the state API rather than 1_000 HTTP
    // requests: what is under test is the handler's behaviour AT the ceiling.
    {
        let state = service_state(&*engine).expect("the in-memory engine must expose this state");
        let mut store = state.lock();
        for index in 0..MAX_STORED_GATEWAYS {
            store.create_gateway(
                REGION,
                "SEEDED-ACTIVATION-KEY",
                &format!("gw-seeded-{index}"),
                BACKUP_VM,
                BTreeMap::new(),
                Creation {
                    by: "SEEDED-CREATOR".to_string(),
                    at: SystemTime::now(),
                },
            );
        }
        assert_eq!(
            store.gateway_count(),
            MAX_STORED_GATEWAYS,
            "case seed: the store must hold the ceiling before the refused create"
        );
    }

    // The create carries an activation the service really issued, so what refuses it is
    // the ceiling and nothing else.
    let activation_key = issued_activation_key(&router).await;
    let refused = call(
        &router,
        "CreateGateway",
        &create_body("gw-over-ceiling", &activation_key, &BTreeMap::new()),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_deref()),
        (400, Some("ValidationException")),
        "case over the ceiling: CreateGateway must be refused with 400 \
         ValidationException, got {} {:?}",
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.str_at("/ErrorCode"),
        None,
        "case over the ceiling: the refusal is the emulator's own bound, not an \
         unredeemable activation, got {:?}",
        refused.body
    );
    assert_eq!(
        service_state(&*engine)
            .expect("state")
            .lock()
            .gateway_count(),
        MAX_STORED_GATEWAYS,
        "case over the ceiling: the refused create must not have grown the store"
    );

    // An unparameterised ListGateways is truncated at the server's default page size.
    let page = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (page.status, page.gateway_arns().len()),
        (200, DEFAULT_PAGE_SIZE),
        "case no MaxResults: the page must be truncated at the server's default size, \
         got {} with {} gateways",
        page.status,
        page.gateway_arns().len()
    );
    assert!(
        page.next_token().is_some(),
        "case no MaxResults: a truncated page must carry a NextToken, got {:?}",
        page.body
    );

    // And a MaxResults larger than the server's maximum is clamped down to it, not
    // honoured.
    let oversized = call(
        &router,
        "ListGateways",
        &json!({ "MaxResults": (MAX_PAGE_SIZE as i64) * 4 }),
    )
    .await;
    assert_eq!(
        (oversized.status, oversized.gateway_arns().len()),
        (200, MAX_PAGE_SIZE),
        "case oversized MaxResults: the page must be clamped to the server maximum, got \
         {} with {} gateways",
        oversized.status,
        oversized.gateway_arns().len()
    );
    assert!(
        oversized.next_token().is_some(),
        "case oversized MaxResults: a truncated page must carry a NextToken, got {:?}",
        oversized.body
    );
}

/// No operation of this service answers the scaffold's 501 any more.
///
/// `UpdateGatewaySoftwareNow` was the last one still scaffolded, so the row that used to
/// assert a 501 for it now asserts that it is SERVED: a 501 from any operation here would
/// mean a handler had been taken back to a scaffold. What that operation does with the
/// record is the sibling `gateway_software_update` suite's subject.
#[tokio::test]
async fn no_operation_of_this_service_answers_501() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-untouched").await;

    let rows: [(&str, Value); 1] = [(
        "UpdateGatewaySoftwareNow",
        json!({ "GatewayArn": arn }),
    )];

    for (operation, body) in rows {
        let reply = call(&router, operation, &body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (200, None),
            "case {operation:?}: an implemented operation must serve the request rather \
             than answer the scaffold's 501, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}
