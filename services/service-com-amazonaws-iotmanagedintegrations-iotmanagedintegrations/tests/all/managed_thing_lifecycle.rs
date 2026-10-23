// SPDX-License-Identifier: CC0-1.0

//! The managed-thing lifecycle, end to end over the wire: `CreateManagedThing` ->
//! `GetManagedThing` -> `ListManagedThings` -> `UpdateManagedThing` ->
//! `DeleteManagedThing`.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`),
//! so the routes, status codes, bodies and modelled errors asserted here are the ones
//! a client sees. The rules under test are stated once and then checked over a TABLE
//! of inputs rather than over one recorded request:
//!
//! * a create stores every member it was given and answers with a generated id, the
//!   managed-thing ARN built from it, and the insert time;
//! * a read projects the STORED record, so what a create or an update wrote comes back;
//! * an update replaces the members present and leaves absent ones alone;
//! * a delete removes the record, so later reads cannot see it;
//! * a list reports the stored records in insertion order, honouring the server-side
//!   filters and paging with an opaque, resumable token.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    IdempotencyKey, IdempotencyRecord, AUTH_MATERIAL_TYPE_VALUES, HUB_NETWORK_MODE_VALUES,
    IDEMPOTENCY_TTL, INITIAL_PROVISIONING_STATUS, MANAGED_THING_PAGE_LIMIT,
    MANAGED_THING_RESOURCE_TYPE, MAX_IDEMPOTENCY_TOKENS, MAX_MANAGED_THINGS, ROLE_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for; the ARNs the service builds carry it.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

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

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine:
/// the same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can seed or
/// inspect the service's store directly instead of driving thousands of requests.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is
/// what routes the request to this service and names the region its ARNs carry.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(router, REGION, method, path_and_query, body).await
}

/// [`call`], signed for an explicit region, so a regional resource can be addressed
/// through another region's endpoint.
async fn call_signed(
    router: &MultiServiceRouter,
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
        .uri(format!("https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
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

/// `CreateManagedThing` on `request`, asserting the modelled 201 and returning the id.
async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let reply = call(router, "POST", "/managed-things", Some(request)).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got body {:?}", reply.body))
        .to_string();
    assert!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')),
        "case {case:?}: Id must match the ManagedThingId pattern ^[a-zA-Z0-9:_-]*$ (len 1..64), \
         got {id:?}"
    );
    assert_eq!(
        reply.string("Arn"),
        Some(
            format!("arn:aws:iotmanagedintegrations:{REGION}:{ACCOUNT}:managed-thing/{id}")
                .as_str()
        ),
        "case {case:?}: Arn must be the managed-thing ARN of the generated id, got {:?}",
        reply.string("Arn")
    );
    assert!(
        reply.seconds("CreatedAt").is_some_and(|secs| secs > 0.0),
        "case {case:?}: expected a CreatedAt timestamp, got {:?}",
        reply.member("CreatedAt")
    );
    id
}

/// The minimal create request: only the three required members.
fn minimal_create(role: &str, material_type: &str, material: &str) -> Value {
    json!({
        "Role": role,
        "AuthenticationMaterialType": material_type,
        "AuthenticationMaterial": material,
    })
}

/// A create request carrying every optional member of the slice, which a read must
/// return unchanged.
fn full_create(name: &str) -> Value {
    json!({
        "Role": "DEVICE",
        "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
        "AuthenticationMaterial": "zwave:qr:0123456789",
        "Owner": "owner.one@example",
        "CredentialLockerId": "locker1",
        "SerialNumber": "SN-0001",
        "Brand": "Acme",
        "Model": "L1",
        "Name": name,
        "Classification": "light bulb",
        "Capabilities": "light:1",
        "MetaData": {"room": "living", "floor": "1"},
        "Tags": {"env": "test", "team": "iot"},
        "WiFiSimpleSetupConfiguration": {
            "EnableAsProvisioner": true,
            "EnableAsProvisionee": false,
            // `TimeoutInMinutes` is modelled range 5..15.
            "TimeoutInMinutes": 7
        },
        "CapabilityReport": {
            // `CapabilityReportVersion` is modelled pattern ^1\.0\.0$.
            "version": "1.0.0",
            "nodeId": "node-1",
            "endpoints": [{
                "id": "1",
                "deviceTypes": ["Dimmer"],
                "capabilities": [{
                    // `SchemaVersionedId` is modelled pattern
                    // ^[a-zA-Z0-9.]+@(\d+\.\d+(\.\d+)?|\$latest)$ and
                    // `CapabilityVersion` pattern ^(0|[1-9][0-9]*)$.
                    "id": "aws.OnOff@1.0",
                    "name": "On/Off",
                    "version": "1",
                    "properties": ["OnOff"],
                    "actions": ["TurnOn"],
                    "events": []
                }]
            }]
        }
    })
}

/// RULE: a create stores what it was given, and a read projects the stored record —
/// over the whole cross product of `Role` x `AuthenticationMaterialType`, plus a
/// request carrying every optional member and one carrying none of them.
#[tokio::test]
async fn a_created_managed_thing_is_read_back_from_stored_state() {
    let router = emulator();

    // Every modelled `Role` x `AuthMaterialType` pair: the create must store the pair
    // it was handed, so the read must report it.
    for &role in ROLE_VALUES {
        for &material_type in AUTH_MATERIAL_TYPE_VALUES {
            let case = format!("{role}/{material_type}");
            let id = create(
                &router,
                &case,
                minimal_create(role, material_type, "qr:0000000000"),
            )
            .await;

            let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetManagedThing, got {} with body {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.string("Id"),
                Some(id.as_str()),
                "case {case:?}: GetManagedThing must report the id it was asked for, got {:?}",
                read.string("Id")
            );
            assert_eq!(
                read.string("Role"),
                Some(role),
                "case {case:?}: expected the stored Role, got {:?}",
                read.string("Role")
            );
            assert_eq!(
                read.string("ProvisioningStatus"),
                Some(INITIAL_PROVISIONING_STATUS),
                "case {case:?}: expected the status the create assigned, got {:?}",
                read.string("ProvisioningStatus")
            );
            // An optional member nothing set must be absent, not an invented value.
            for absent in ["Owner", "Name", "Brand", "Model", "SerialNumber", "MetaData"] {
                assert!(
                    read.member(absent).is_none(),
                    "case {case:?}: {absent} was never set, so it must be absent, got {:?}",
                    read.member(absent)
                );
            }
            // `AuthenticationMaterial` is `@sensitive` and no output shape declares it.
            assert!(
                read.member("AuthenticationMaterial").is_none(),
                "case {case:?}: no modelled output member carries AuthenticationMaterial, got {:?}",
                read.member("AuthenticationMaterial")
            );
        }
    }

    // A request carrying every optional member of the slice: each one must survive the
    // round trip.
    let id = create(&router, "full", full_create("living-room lamp")).await;
    let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case \"full\": expected HTTP 200, got {} with body {:?}",
        read.status, read.body
    );
    for (member, expected) in [
        ("Owner", "owner.one@example"),
        ("CredentialLockerId", "locker1"),
        ("SerialNumber", "SN-0001"),
        ("Brand", "Acme"),
        ("Model", "L1"),
        ("Name", "living-room lamp"),
        ("Classification", "light bulb"),
    ] {
        assert_eq!(
            read.string(member),
            Some(expected),
            "case \"full\": expected the stored {member}, got {:?}",
            read.string(member)
        );
    }
    assert_eq!(
        read.member("MetaData"),
        Some(&json!({"room": "living", "floor": "1"})),
        "case \"full\": expected the stored MetaData, got {:?}",
        read.member("MetaData")
    );
    assert_eq!(
        read.member("Tags"),
        Some(&json!({"env": "test", "team": "iot"})),
        "case \"full\": expected the stored Tags, got {:?}",
        read.member("Tags")
    );
    assert_eq!(
        read.member("WiFiSimpleSetupConfiguration"),
        Some(&json!({
            "EnableAsProvisioner": true,
            "EnableAsProvisionee": false,
            "TimeoutInMinutes": 7
        })),
        "case \"full\": expected the stored WiFiSimpleSetupConfiguration, got {:?}",
        read.member("WiFiSimpleSetupConfiguration")
    );
    assert_eq!(
        read.seconds("UpdatedAt"),
        read.seconds("CreatedAt"),
        "case \"full\": an unmodified record's UpdatedAt is its CreatedAt, got {:?} and {:?}",
        read.member("UpdatedAt"),
        read.member("CreatedAt")
    );
    assert!(
        read.member("ActivatedAt").is_none(),
        "case \"full\": nothing activated the thing, so ActivatedAt must be absent, got {:?}",
        read.member("ActivatedAt")
    );
}

/// RULE: every create mints a distinct id and stores a distinct record under it, except
/// a replay of the same `@idempotencyToken`, which answers with the first record.
#[tokio::test]
async fn ids_are_distinct_and_a_replayed_client_token_returns_the_first_record() {
    let router = emulator();

    let mut ids = BTreeSet::new();
    for case in 0..5u32 {
        let id = create(
            &router,
            &format!("distinct-{case}"),
            minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:0000000000"),
        )
        .await;
        assert!(
            ids.insert(id.clone()),
            "case {case}: every create must mint a fresh id, got the already-used {id:?}"
        );
    }

    for token in ["token-1", "TOKEN_2", "t-3=="] {
        let mut request = minimal_create("CONTROLLER", "WIFI_SETUP_QR_BAR_CODE", "qr:1111111111");
        request["ClientToken"] = json!(token);
        request["Name"] = json!("first");

        let first = call(&router, "POST", "/managed-things", Some(request.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {token:?}: expected HTTP 201, got {} with body {:?}",
            first.status, first.body
        );

        // A retry with the same token and parameters performs no further action.
        request["Name"] = json!("second");
        let replay = call(&router, "POST", "/managed-things", Some(request)).await;
        assert_eq!(
            replay.status, 201,
            "case {token:?}: expected HTTP 201 on the replay, got {} with body {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.string("Id"),
            first.string("Id"),
            "case {token:?}: a replayed ClientToken must answer with the first id, got {:?} after \
             {:?}",
            replay.string("Id"),
            first.string("Id")
        );
        assert_eq!(
            replay.seconds("CreatedAt"),
            first.seconds("CreatedAt"),
            "case {token:?}: a replay must report the original CreatedAt, got {:?} after {:?}",
            replay.member("CreatedAt"),
            first.member("CreatedAt")
        );

        let read = call(
            &router,
            "GET",
            &format!("/managed-things/{}", first.string("Id").unwrap_or_default()),
            None,
        )
        .await;
        assert_eq!(
            read.string("Name"),
            Some("first"),
            "case {token:?}: the replay must not have overwritten the stored record, got {:?}",
            read.string("Name")
        );
    }
}

/// RULE: an `Identifier` the store does not hold is `ResourceNotFoundException` (404)
/// on every operation that resolves one.
#[tokio::test]
async fn an_unknown_identifier_is_resource_not_found() {
    let router = emulator();
    // A live record, so a 404 can only come from resolving the identifier.
    let live = create(
        &router,
        "live",
        minimal_create("DEVICE", "ZIGBEE_QR_BAR_CODE", "qr:2222222222"),
    )
    .await;

    for unknown in ["does-not-exist", "a", "A:B_C-1", &format!("{live}x")] {
        for (method, body) in [
            ("GET", None),
            ("PUT", Some(json!({"Name": "whatever"}))),
            ("DELETE", None),
        ] {
            let case = format!("{method} {unknown}");
            let reply = call(
                &router,
                method,
                &format!("/managed-things/{unknown}"),
                body.clone(),
            )
            .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404 for an unknown identifier, got {} with body {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
                reply.error_type
            );
        }
    }
}

/// RULE: an update replaces each member the request carries and leaves every absent
/// member untouched, and the record's `UpdatedAt` moves to the time of the update.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    // One row per updatable member of the slice, so each is proven to reach the store.
    let rows: Vec<(&str, Value, &str, Value)> = vec![
        ("Name", json!({"Name": "den lamp"}), "Name", json!("den lamp")),
        ("Owner", json!({"Owner": "owner.two"}), "Owner", json!("owner.two")),
        (
            "CredentialLockerId",
            json!({"CredentialLockerId": "locker2"}),
            "CredentialLockerId",
            json!("locker2"),
        ),
        (
            "SerialNumber",
            json!({"SerialNumber": "SN-0002"}),
            "SerialNumber",
            json!("SN-0002"),
        ),
        ("Brand", json!({"Brand": "Globex"}), "Brand", json!("Globex")),
        ("Model", json!({"Model": "L2"}), "Model", json!("L2")),
        (
            "Classification",
            json!({"Classification": "thermostat"}),
            "Classification",
            json!("thermostat"),
        ),
        (
            "HubNetworkMode",
            json!({"HubNetworkMode": HUB_NETWORK_MODE_VALUES[1]}),
            "HubNetworkMode",
            json!(HUB_NETWORK_MODE_VALUES[1]),
        ),
        (
            "MetaData",
            json!({"MetaData": {"room": "den"}}),
            "MetaData",
            json!({"room": "den"}),
        ),
        (
            "WiFiSimpleSetupConfiguration",
            json!({"WiFiSimpleSetupConfiguration": {"EnableAsProvisionee": true, "TimeoutInMinutes": 15}}),
            "WiFiSimpleSetupConfiguration",
            json!({"EnableAsProvisionee": true, "TimeoutInMinutes": 15}),
        ),
        // An update that carries no updatable member still bumps UpdatedAt and changes
        // nothing else.
        ("no-op", json!({}), "Name", json!("living-room lamp")),
    ];

    for (case, patch, observed, expected) in rows {
        let router = emulator();
        let id = create(&router, case, full_create("living-room lamp")).await;
        let before = call(&router, "GET", &format!("/managed-things/{id}"), None).await;

        // The clock must be able to separate the create from the update.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let update = call(
            &router,
            "PUT",
            &format!("/managed-things/{id}"),
            Some(patch.clone()),
        )
        .await;
        assert_eq!(
            update.status, 204,
            "case {case:?}: expected HTTP 204 from UpdateManagedThing, got {} with body {:?}",
            update.status, update.body
        );
        assert_eq!(
            update.body,
            Value::Null,
            "case {case:?}: UpdateManagedThing answers with an empty body, got {:?}",
            update.body
        );

        let after = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
        assert_eq!(
            after.member(observed),
            Some(&expected),
            "case {case:?}: expected {observed} to read back as {expected:?}, got {:?}",
            after.member(observed)
        );

        // Every member the patch did not carry is unchanged.
        for member in [
            "Id",
            "Arn",
            "Role",
            "ProvisioningStatus",
            "CreatedAt",
            "Owner",
            "CredentialLockerId",
            "SerialNumber",
            "Brand",
            "Model",
            "Name",
            "Classification",
            "MetaData",
            "Tags",
            "WiFiSimpleSetupConfiguration",
        ] {
            if patch.get(member).is_some() {
                continue;
            }
            assert_eq!(
                after.member(member),
                before.member(member),
                "case {case:?}: {member} was absent from the update, so it must be unchanged: \
                 was {:?}, now {:?}",
                before.member(member),
                after.member(member)
            );
        }

        let (was, now) = (
            before.seconds("UpdatedAt").unwrap_or_default(),
            after.seconds("UpdatedAt").unwrap_or_default(),
        );
        assert!(
            now > was,
            "case {case:?}: the update must bump UpdatedAt, was {was} and now {now}"
        );
        assert!(
            now >= after.seconds("CreatedAt").unwrap_or_default(),
            "case {case:?}: UpdatedAt must not precede CreatedAt, got {now} against {:?}",
            after.member("CreatedAt")
        );
    }
}

/// RULE: a delete removes the record, so a later read answers 404 and a later list no
/// longer reports it — whatever the `Force` flag says.
#[tokio::test]
async fn a_deleted_managed_thing_is_gone_from_every_read() {
    for (case, query) in [
        ("force-absent", ""),
        ("force-true", "?Force=true"),
        ("force-false", "?Force=false"),
    ] {
        let router = emulator();
        let doomed = create(&router, case, full_create("doomed lamp")).await;
        let kept = create(
            &router,
            case,
            minimal_create("CONTROLLER", "PRE_ONBOARDED_CLOUD", "qr:3333333333"),
        )
        .await;

        let delete = call(
            &router,
            "DELETE",
            &format!("/managed-things/{doomed}{query}"),
            None,
        )
        .await;
        assert_eq!(
            delete.status, 204,
            "case {case:?}: expected HTTP 204 from DeleteManagedThing, got {} with body {:?}",
            delete.status, delete.body
        );

        let read = call(&router, "GET", &format!("/managed-things/{doomed}"), None).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted managed thing must read as 404, got {} with body {:?}",
            read.status, read.body
        );

        let list = call(&router, "GET", "/managed-things", None).await;
        let listed = ids_of(&list);
        assert!(
            !listed.contains(&doomed) && listed.contains(&kept),
            "case {case:?}: the list must drop the deleted id {doomed:?} and keep {kept:?}, got \
             {listed:?}"
        );
    }
}

/// The ids a `ListManagedThings` reply reports, in reply order.
fn ids_of(reply: &Reply) -> Vec<String> {
    reply
        .member("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a list reports the stored records in insertion order, and `MaxResults` cuts the
/// page at that many items with an opaque `NextToken` that resumes exactly where the
/// page stopped. An empty store answers an empty page with no token.
#[tokio::test]
async fn a_list_pages_over_the_store_in_insertion_order() {
    let router = emulator();

    let empty = call(&router, "GET", "/managed-things", None).await;
    assert_eq!(
        empty.status, 200,
        "case \"empty\": expected HTTP 200, got {} with body {:?}",
        empty.status, empty.body
    );
    assert!(
        ids_of(&empty).is_empty() && empty.member("NextToken").is_none(),
        "case \"empty\": an empty store answers an empty page with no NextToken, got {:?}",
        empty.body
    );

    // Seven records, so the page sizes below divide the store evenly and unevenly.
    let mut inserted = Vec::new();
    for n in 0..7u32 {
        inserted.push(
            create(
                &router,
                &format!("seed-{n}"),
                minimal_create(
                    ROLE_VALUES[(n as usize) % ROLE_VALUES.len()],
                    AUTH_MATERIAL_TYPE_VALUES[(n as usize) % AUTH_MATERIAL_TYPE_VALUES.len()],
                    "qr:4444444444",
                ),
            )
            .await,
        );
    }

    let unpaged = call(&router, "GET", "/managed-things", None).await;
    assert_eq!(
        ids_of(&unpaged),
        inserted,
        "case \"unpaged\": a list with no MaxResults reports every record in insertion order"
    );
    assert!(
        unpaged.member("NextToken").is_none(),
        "case \"unpaged\": a complete page carries no NextToken, got {:?}",
        unpaged.member("NextToken")
    );

    // Each page size: walking the token chain must visit every id exactly once, in
    // insertion order, and the last page must carry no token. 1 and 1000 are the
    // modelled bounds of MaxResults (range 1..1000).
    for page_size in [1usize, 2, 3, 7, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= inserted.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let query = match &token {
                None => format!("/managed-things?MaxResults={page_size}"),
                Some(next) => format!("/managed-things?MaxResults={page_size}&NextToken={next}"),
            };
            let page = call(&router, "GET", &query, None).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                page.status, page.body
            );
            let page_ids = ids_of(&page);
            assert!(
                page_ids.len() <= page_size,
                "case {case:?}: a page must hold at most MaxResults items, got {} in {page_ids:?}",
                page_ids.len()
            );
            seen.extend(page_ids);
            match page.string("NextToken") {
                Some(next) => {
                    assert!(
                        !next.is_empty()
                            && next.chars().all(|c| c.is_ascii_alphanumeric()
                                || matches!(c, '=' | '_' | '-')),
                        "case {case:?}: NextToken must match ^[a-zA-Z0-9=_-]+$, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, inserted,
            "case {case:?}: the token chain must report every record exactly once, in insertion \
             order"
        );
    }

    // `MaxResults` is modelled range 1..1000, so 0 is outside it: it must be refused or
    // clamped to a page that advances — never an empty page with a NextToken, which a
    // client could follow forever without making progress.
    let zero = call(&router, "GET", "/managed-things?MaxResults=0", None).await;
    assert!(
        zero.status == 400 || !ids_of(&zero).is_empty(),
        "case \"MaxResults=0\": expected a refusal or a non-empty page, got {} with body {:?}",
        zero.status,
        zero.body
    );

    // A token this service never issued is refused, not answered with a page.
    for bogus_token in [
        "not-a-real-token",
        "deadbeef",
        "zzzzzzzzzzzzzzzz",
        // Well-formed but beyond anything this store has minted.
        "00000000ffffffff",
    ] {
        let bogus = call(
            &router,
            "GET",
            &format!("/managed-things?NextToken={bogus_token}"),
            None,
        )
        .await;
        assert_eq!(
            bogus.status, 400,
            "case {bogus_token:?}: an unissued NextToken is refused, got {} with body {:?}",
            bogus.status, bogus.body
        );
        assert_eq!(
            bogus.error_type.as_deref(),
            Some("ValidationException"),
            "case {bogus_token:?}: expected the modelled ValidationException, got {:?}",
            bogus.error_type
        );
    }

    // A cursor at the end of what this store has minted is an empty final page, not an
    // error: the seven creates above consumed sequences 0..=6.
    let past_the_end = call(
        &router,
        "GET",
        "/managed-things?NextToken=0000000000000007",
        None,
    )
    .await;
    assert_eq!(
        past_the_end.status, 200,
        "case \"past-the-end\": expected HTTP 200, got {} with body {:?}",
        past_the_end.status, past_the_end.body
    );
    assert!(
        ids_of(&past_the_end).is_empty() && past_the_end.member("NextToken").is_none(),
        "case \"past-the-end\": a cursor past the last record is an empty final page, got {:?}",
        past_the_end.body
    );
}

/// RULE: one list response is bounded: it carries at most
/// `MANAGED_THING_PAGE_LIMIT` summaries however many records are stored and whatever
/// `MaxResults` asks for, and the rest are reachable through the `NextToken`.
#[tokio::test]
async fn a_list_response_is_bounded_by_the_page_limit() {
    let router = emulator();

    // More records than one page may carry.
    let stored = MANAGED_THING_PAGE_LIMIT + 17;
    let mut inserted = Vec::with_capacity(stored);
    for n in 0..stored {
        inserted.push(
            create(
                &router,
                &format!("bulk-{n}"),
                minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:7777777777"),
            )
            .await,
        );
    }

    // An absent MaxResults, and one asking for more than the limit (including the
    // modelled maximum of 1000), are all capped at the limit.
    for case in ["", "?MaxResults=1000", "?MaxResults=500"] {
        let page = call(&router, "GET", &format!("/managed-things{case}"), None).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            page.status, page.body
        );
        let ids = ids_of(&page);
        assert_eq!(
            ids.len(),
            MANAGED_THING_PAGE_LIMIT,
            "case {case:?}: a page carries at most {MANAGED_THING_PAGE_LIMIT} items, got {}",
            ids.len()
        );
        assert_eq!(
            ids,
            inserted[..MANAGED_THING_PAGE_LIMIT],
            "case {case:?}: the first page must be the oldest {MANAGED_THING_PAGE_LIMIT} records, \
             in insertion order"
        );
        let token = page.string("NextToken").unwrap_or_default().to_string();
        assert!(
            !token.is_empty(),
            "case {case:?}: a truncated page must carry a NextToken, got body {:?}",
            page.body
        );

        let rest = call(
            &router,
            "GET",
            &format!("/managed-things?NextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            ids_of(&rest),
            inserted[MANAGED_THING_PAGE_LIMIT..],
            "case {case:?}: the token must resume exactly where the page stopped"
        );
        assert!(
            rest.member("NextToken").is_none(),
            "case {case:?}: the final page carries no NextToken, got {:?}",
            rest.member("NextToken")
        );
    }
}

/// RULE: a managed thing is a regional resource — only the region whose endpoint
/// created it resolves, lists, updates or deletes it.
#[tokio::test]
async fn a_managed_thing_belongs_to_the_region_that_created_it() {
    let router = emulator();

    // One record per region, each created through that region's endpoint.
    let mut per_region = Vec::new();
    for region in ["us-east-1", "eu-west-1", "ap-south-1"] {
        let reply = call_signed(
            &router,
            region,
            "POST",
            "/managed-things",
            Some(minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:8888888888")),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {region:?}: expected HTTP 201, got {} with body {:?}",
            reply.status, reply.body
        );
        let id = reply.string("Id").unwrap_or_default().to_string();
        assert_eq!(
            reply.string("Arn"),
            Some(
                format!("arn:aws:iotmanagedintegrations:{region}:{ACCOUNT}:managed-thing/{id}")
                    .as_str()
            ),
            "case {region:?}: the ARN must carry the region the request was signed for, got {:?}",
            reply.string("Arn")
        );
        per_region.push((region, id));
    }

    for (owning, id) in &per_region {
        // The owning region sees exactly its own record.
        let own_list = call_signed(&router, owning, "GET", "/managed-things", None).await;
        assert_eq!(
            ids_of(&own_list),
            vec![id.clone()],
            "case {owning:?}: a list reports only this region's records"
        );

        for (other, _) in per_region.iter().filter(|(other, _)| other != owning) {
            let case = format!("{id} via {other}");
            for (method, body) in [
                ("GET", None),
                ("PUT", Some(json!({"Name": "renamed"}))),
                ("DELETE", None),
            ] {
                let reply = call_signed(
                    &router,
                    other,
                    method,
                    &format!("/managed-things/{id}"),
                    body,
                )
                .await;
                assert_eq!(
                    reply.status, 404,
                    "case {case:?}: {method} through another region must not resolve the record, \
                     got {} with body {:?}",
                    reply.status, reply.body
                );
            }
        }
    }

    // None of those cross-region attempts changed anything.
    for (owning, id) in &per_region {
        let read = call_signed(&router, owning, "GET", &format!("/managed-things/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {owning:?}: the owning region still resolves its record, got {} with body {:?}",
            read.status, read.body
        );
        assert!(
            read.member("Name").is_none(),
            "case {owning:?}: a cross-region update must not have touched the record, got {:?}",
            read.member("Name")
        );
    }
}

/// RULE: the store is bounded — a create past `MAX_MANAGED_THINGS` is refused with a
/// modelled error instead of growing the store, and deleting a record makes room again.
#[tokio::test]
async fn the_store_refuses_a_create_past_its_maximum() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // One real create, then fill the store to its maximum from that record, so the
    // boundary is exercised without driving MAX_MANAGED_THINGS requests.
    let seed = create(&router, "seed", full_create("bound lamp")).await;
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let template = store
            .managed_things
            .get(&seed)
            .cloned()
            .expect("the seeded record is stored");
        while store.managed_things.len() < MAX_MANAGED_THINGS {
            let sequence = store.take_sequence();
            let id = format!("filler-{sequence}");
            let mut filler = template.clone();
            filler.id = id.clone();
            filler.arn = format!("{}-{sequence}", template.arn);
            filler.sequence = sequence;
            store.managed_things.insert(id.clone(), filler);
            store.managed_thing_order.push(id);
        }
    }

    let refused = call(
        &router,
        "POST",
        "/managed-things",
        Some(minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:9999999999")),
    )
    .await;
    assert_eq!(
        refused.status, 429,
        "case \"at-the-maximum\": a create past the store's maximum must be refused, got {} with \
         body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ThrottlingException"),
        "case \"at-the-maximum\": expected a modelled refusal, got {:?}",
        refused.error_type
    );
    assert_eq!(
        engine.state::<ServiceState>().lock().managed_things.len(),
        MAX_MANAGED_THINGS,
        "case \"at-the-maximum\": a refused create must not have grown the store"
    );

    // Deleting one record makes room for exactly one more.
    let delete = call(&router, "DELETE", &format!("/managed-things/{seed}"), None).await;
    assert_eq!(
        delete.status, 204,
        "case \"make-room\": expected HTTP 204, got {} with body {:?}",
        delete.status, delete.body
    );
    create(
        &router,
        "make-room",
        minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:9999999999"),
    )
    .await;
}

/// RULE: an idempotency record does not outlive its TTL, and the map never holds more
/// than `MAX_IDEMPOTENCY_TOKENS` records — so a caller minting tokens cannot make the
/// store grow without end.
#[tokio::test]
async fn idempotency_records_are_retired_by_their_ttl_and_capped() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let kept = create(&router, "kept", full_create("ttl lamp")).await;

    // Seed the map to its cap: half already past the TTL, half minted just now.
    let (expired_tokens, fresh_tokens) = {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        let now = SystemTime::now();
        let mut expired = Vec::new();
        let mut fresh = Vec::new();
        for n in 0..MAX_IDEMPOTENCY_TOKENS {
            let token = format!("seeded-{n}");
            let past_ttl = n % 2 == 0;
            let minted_at = if past_ttl {
                now - IDEMPOTENCY_TTL - Duration::from_secs(1)
            } else {
                now
            };
            store.idempotency_tokens.insert(
                IdempotencyKey::new(MANAGED_THING_RESOURCE_TYPE, &token),
                IdempotencyRecord {
                    resource_id: kept.clone(),
                    minted_at,
                },
            );
            if past_ttl {
                expired.push(token);
            } else {
                fresh.push(token);
            }
        }
        (expired, fresh)
    };

    // One create retires what the clock already made due before minting its own token.
    let mut request = minimal_create("DEVICE", "ZWAVE_QR_BAR_CODE", "qr:1010101010");
    request["ClientToken"] = json!("after-the-sweep");
    let accepted = call(&router, "POST", "/managed-things", Some(request)).await;
    assert_eq!(
        accepted.status, 201,
        "case \"sweep\": expected HTTP 201, got {} with body {:?}",
        accepted.status, accepted.body
    );

    let store = engine.state::<ServiceState>();
    let tokens = &store.lock().idempotency_tokens;
    assert!(
        tokens.len() <= MAX_IDEMPOTENCY_TOKENS,
        "case \"sweep\": the map must stay within {MAX_IDEMPOTENCY_TOKENS} records, got {}",
        tokens.len()
    );
    for token in &expired_tokens {
        assert!(
            !tokens.contains_key(&IdempotencyKey::new(MANAGED_THING_RESOURCE_TYPE, token)),
            "case \"sweep\": the record for {token:?} was past its TTL, so it must be retired"
        );
    }
    let surviving_fresh = fresh_tokens
        .iter()
        .filter(|token| {
            tokens.contains_key(&IdempotencyKey::new(MANAGED_THING_RESOURCE_TYPE, token))
        })
        .count();
    assert!(
        surviving_fresh >= fresh_tokens.len() - 1,
        "case \"sweep\": a record inside its TTL must survive, {surviving_fresh} of {} did",
        fresh_tokens.len()
    );
    assert!(
        tokens.contains_key(&IdempotencyKey::new(
            MANAGED_THING_RESOURCE_TYPE,
            "after-the-sweep"
        )),
        "case \"sweep\": the create's own token must be recorded, got {} records",
        tokens.len()
    );
}

/// RULE: each list filter selects exactly the stored records whose corresponding member
/// equals the filter value; a record that does not carry the member matches no value.
#[tokio::test]
async fn list_filters_select_on_stored_members() {
    let router = emulator();

    // Three records that differ in every filterable member.
    let mut lamp = full_create("filter lamp");
    lamp["Owner"] = json!("owner.alpha");
    lamp["SerialNumber"] = json!("SN-AAA");
    lamp["CredentialLockerId"] = json!("lockeralpha");
    let lamp_id = create(&router, "lamp", lamp).await;

    let mut hub = full_create("filter hub");
    hub["Role"] = json!("CONTROLLER");
    hub["Owner"] = json!("owner.beta");
    hub["SerialNumber"] = json!("SN-BBB");
    hub["CredentialLockerId"] = json!("lockerbeta");
    let hub_id = create(&router, "hub", hub).await;

    let bare_id = create(
        &router,
        "bare",
        minimal_create("DEVICE", "DISCOVERED_DEVICE", "qr:5555555555"),
    )
    .await;

    let rows: Vec<(String, Vec<String>)> = vec![
        ("OwnerFilter=owner.alpha".to_string(), vec![lamp_id.clone()]),
        ("OwnerFilter=owner.beta".to_string(), vec![hub_id.clone()]),
        ("OwnerFilter=owner.absent".to_string(), vec![]),
        ("RoleFilter=CONTROLLER".to_string(), vec![hub_id.clone()]),
        (
            "RoleFilter=DEVICE".to_string(),
            vec![lamp_id.clone(), bare_id.clone()],
        ),
        ("SerialNumberFilter=SN-AAA".to_string(), vec![lamp_id.clone()]),
        ("SerialNumberFilter=SN-BBB".to_string(), vec![hub_id.clone()]),
        (
            "CredentialLockerFilter=lockeralpha".to_string(),
            vec![lamp_id.clone()],
        ),
        (
            format!("ProvisioningStatusFilter={INITIAL_PROVISIONING_STATUS}"),
            vec![lamp_id.clone(), hub_id.clone(), bare_id.clone()],
        ),
        // A status no record carries selects nothing.
        ("ProvisioningStatusFilter=ACTIVATED".to_string(), vec![]),
        // No operation in this slice associates a parent controller or a connector, so
        // no record carries one and a filter on it selects nothing.
        (
            "ParentControllerIdentifierFilter=anycontroller".to_string(),
            vec![],
        ),
        (
            "ConnectorDestinationIdFilter=any-destination".to_string(),
            vec![],
        ),
        ("ConnectorDeviceIdFilter=any.device".to_string(), vec![]),
        // Two filters apply together.
        (
            "RoleFilter=DEVICE&SerialNumberFilter=SN-AAA".to_string(),
            vec![lamp_id.clone()],
        ),
        (
            "RoleFilter=CONTROLLER&SerialNumberFilter=SN-AAA".to_string(),
            vec![],
        ),
    ];

    for (query, expected) in rows {
        let reply = call(&router, "GET", &format!("/managed-things?{query}"), None).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected HTTP 200, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            ids_of(&reply),
            expected,
            "case {query:?}: the filter must select exactly the matching stored records"
        );
    }
}

/// RULE: a value outside a modelled enum is refused as a client error, and nothing is
/// stored for the refused request.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused() {
    let router = emulator();

    let rows: Vec<(&str, Value)> = vec![
        (
            "Role",
            json!({
                "Role": "ROUTER",
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:6666666666"
            }),
        ),
        (
            "AuthenticationMaterialType",
            json!({
                "Role": "DEVICE",
                "AuthenticationMaterialType": "BLUETOOTH_QR_BAR_CODE",
                "AuthenticationMaterial": "qr:6666666666"
            }),
        ),
    ];

    for (case, request) in rows {
        let reply = call(&router, "POST", "/managed-things", Some(request)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unmodelled enum value is a 400, got {} with body {:?}",
            reply.status, reply.body
        );
    }

    // An invalid HubNetworkMode is refused the same way, and the stored record is
    // untouched.
    let id = create(&router, "hub-mode", full_create("mode lamp")).await;
    let refused = call(
        &router,
        "PUT",
        &format!("/managed-things/{id}"),
        Some(json!({"HubNetworkMode": "MESH", "Name": "renamed"})),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"hub-mode\": an unmodelled HubNetworkMode is a 400, got {} with body {:?}",
        refused.status, refused.body
    );

    let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        read.string("Name"),
        Some("mode lamp"),
        "case \"hub-mode\": a refused update must store nothing, got {:?}",
        read.string("Name")
    );

    let list = call(&router, "GET", "/managed-things", None).await;
    assert_eq!(
        ids_of(&list),
        vec![id],
        "case \"refused\": only the one accepted create may appear in the store, got {:?}",
        list.body
    );
}
