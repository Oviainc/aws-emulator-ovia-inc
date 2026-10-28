// SPDX-License-Identifier: CC0-1.0

//! Moving one dedicated IP address into a dedicated IP pool, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised. The pools come from `CreateDedicatedIpPool`, and the addresses are
//! seeded into the engine's own state, because SES allocates a dedicated IP out of band
//! and no request of this API mints one.
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `PutDedicatedIpInPool` PERSISTS the destination pool the request names onto the
//!    account's record of the address the request names, REPLACING whatever pool that
//!    record was in and leaving every other attribute of the address — and every other
//!    address — exactly as it was, so the address's own read (`GetDedicatedIp`) and the
//!    listing (`GetDedicatedIps`) then report it in the destination pool and nowhere
//!    else; repeating the same move answers the same way and changes nothing further;
//! 2. either resource the move needs being absent is a missing resource
//!    (`NotFoundException`, 404) that persists nothing: an address the account holds no
//!    dedicated IP for, and a destination pool the account does not hold;
//! 3. a value that is not an IPv4 address at all is invalid input
//!    (`BadRequestException`, 400), and that refusal comes before either resource is
//!    looked up.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, DedicatedIpRecord, MANAGED_WARMUP_PERCENTAGE,
    WARMUP_COMPLETE_PERCENTAGE, WARMUP_START_PERCENTAGE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `DedicatedIp` a read reports, or `Value::Null` when it reports none.
    fn dedicated_ip(&self) -> Value {
        self.body
            .get("DedicatedIp")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The addresses a listing reports, by `Ip`; an absent list reads as empty.
    fn listed_ips(&self) -> Vec<String> {
        self.body
            .get("DedicatedIps")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("Ip").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can put a
/// dedicated IP in the account the way SES's out-of-band allocation does.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let region = "us-east-1";
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
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

    /// `PutDedicatedIpInPool`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="PutDedicatedIpInPool")`:
    /// `PUT /v2/email/dedicated-ips/{Ip}/pool`, with `DestinationPoolName` in the body).
    async fn put_in_pool(&self, ip: &str, destination_pool_name: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/dedicated-ips/{ip}/pool"),
            Some(json!({ "DestinationPoolName": destination_pool_name })),
        )
        .await
    }

    /// `GetDedicatedIp`'s route: `GET /v2/email/dedicated-ips/{Ip}`.
    async fn get_dedicated_ip(&self, ip: &str) -> Reply {
        self.call("GET", &format!("/v2/email/dedicated-ips/{ip}"), None)
            .await
    }

    /// `GetDedicatedIps`'s route: `GET /v2/email/dedicated-ips`, narrowed to one pool.
    async fn get_dedicated_ips_in_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/dedicated-ips?PoolName={pool_name}"),
            None,
        )
        .await
    }

    /// `CreateDedicatedIpPool`'s route: `POST /v2/email/dedicated-ip-pools`.
    async fn create_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/dedicated-ip-pools",
            Some(json!({ "PoolName": pool_name })),
        )
        .await
    }

    /// Put a dedicated IP in the account: no SES request allocates one, so a case that
    /// moves one writes the record the allocation would have left.
    fn allocate(&self, ip: &str, pool_name: Option<&str>, warmup_percentage: i32) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().dedicated_ips.insert(
            ip.to_string(),
            DedicatedIpRecord::new(ip, pool_name.map(str::to_string), warmup_percentage),
        );
    }

    /// The dedicated IPs the account holds, as the store has them: what an assertion
    /// about what a call did or did not change compares against.
    fn stored(&self) -> Vec<(String, Option<String>, i32)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .dedicated_ips
            .values()
            .map(|record| {
                (
                    record.ip.clone(),
                    record.pool_name.clone(),
                    record.warmup_percentage,
                )
            })
            .collect()
    }
}

/// One row of the move table: an address the account holds, the pool it was in, the
/// warm-up it was holding, and the pool the request moves it to.
struct MoveCase {
    case: &'static str,
    ip: &'static str,
    held_pool: Option<&'static str>,
    warmup: i32,
    destination: &'static str,
}

/// Every row of the move table: the rows differ along where the address starts (in no
/// pool, in another pool, in the destination pool already), the form of the destination
/// pool's name, the warm-up the address carries (both bounds
/// `DedicatedIp$WarmupPercentage` publishes, a value between them, and the value that
/// does not apply) and the form of the address.
fn move_cases() -> Vec<MoveCase> {
    vec![
        MoveCase {
            case: "an address in no pool at all moved into one",
            ip: "198.51.100.1",
            held_pool: None,
            warmup: WARMUP_START_PERCENTAGE,
            destination: "marketing-pool",
        },
        MoveCase {
            case: "an address moved out of one pool into another",
            ip: "198.51.100.23",
            held_pool: Some("marketing-pool"),
            warmup: 37,
            destination: "transactional_pool",
        },
        MoveCase {
            case: "an address moved into the pool it is already in",
            ip: "198.51.100.254",
            held_pool: Some("transactional_pool"),
            warmup: WARMUP_COMPLETE_PERCENTAGE,
            destination: "transactional_pool",
        },
        MoveCase {
            case: "a fully warmed-up address moved into a one-character pool",
            ip: "192.0.2.8",
            held_pool: None,
            warmup: WARMUP_COMPLETE_PERCENTAGE,
            destination: "p",
        },
        MoveCase {
            case: "an address whose warm-up does not apply",
            ip: "203.0.113.9",
            held_pool: Some("managed-pool"),
            warmup: MANAGED_WARMUP_PERCENTAGE,
            destination: "edge-pool",
        },
        MoveCase {
            case: "the lowest address form, into a pool named with digits",
            ip: "10.0.0.1",
            held_pool: Some("edge-pool"),
            warmup: 1,
            destination: "pool-2026",
        },
        MoveCase {
            case: "the highest address form, into a long pool name",
            ip: "255.255.255.255",
            held_pool: None,
            warmup: WARMUP_COMPLETE_PERCENTAGE - 1,
            destination: "a-rather-long-dedicated-ip-pool-name-for-one-account",
        },
    ]
}

/// The `WarmupStatus` an address carrying `warmup_percentage` reports, as
/// `DedicatedIp$WarmupStatus` and `DedicatedIp$WarmupPercentage` document it together: a
/// percentage that does not apply is `NOT_APPLICABLE`, `100` is `DONE`, below that is
/// `IN_PROGRESS`.
fn warmup_status(warmup_percentage: i32) -> &'static str {
    if warmup_percentage < WARMUP_START_PERCENTAGE {
        "NOT_APPLICABLE"
    } else if warmup_percentage >= WARMUP_COMPLETE_PERCENTAGE {
        "DONE"
    } else {
        "IN_PROGRESS"
    }
}

/// The `DedicatedIp` document a read of `case` must answer with once the move has
/// landed: the destination pool, and every other attribute as it was.
fn expected_document(case: &MoveCase) -> Value {
    json!({
        "Ip": case.ip,
        "WarmupStatus": warmup_status(case.warmup),
        "WarmupPercentage": case.warmup,
        "PoolName": case.destination,
    })
}

/// Create every pool a row of the table starts in or moves to, so each move names a pool
/// that exists.
async fn create_every_pool(ses: &Ses, cases: &[MoveCase]) {
    let mut names: Vec<&str> = Vec::new();
    for case in cases {
        for name in case.held_pool.into_iter().chain([case.destination]) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in names {
        let created = ses.create_pool(name).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected the pool to be created with 200, got {} {:?}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn a_move_persists_the_destination_pool_onto_the_address_it_names() {
    let ses = Ses::new();
    let cases = move_cases();
    create_every_pool(&ses, &cases).await;
    // Every address is in the account at once, so each move has to pick out the record
    // the request names rather than the only one there is.
    for case in &cases {
        ses.allocate(case.ip, case.held_pool, case.warmup);
    }

    for case in &cases {
        let moved = ses.put_in_pool(case.ip, case.destination).await;
        assert_eq!(
            moved.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, moved.status, moved.body
        );

        // The round trip: the address's own read reports the destination pool, and
        // nothing else about the address moved with it.
        let read = ses.get_dedicated_ip(case.ip).await;
        assert_eq!(
            read.dedicated_ip(),
            expected_document(case),
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            expected_document(case),
            read.dedicated_ip()
        );
    }

    // And the listing narrowed to each destination pool reports exactly the addresses
    // this table moved there — an address is in one pool at a time, so a move out of a
    // pool takes the address off that pool's listing too.
    for case in &cases {
        let expected: Vec<String> = cases
            .iter()
            .filter(|row| row.destination == case.destination)
            .map(|row| row.ip.to_string())
            .collect();
        let listed = ses.get_dedicated_ips_in_pool(case.destination).await;
        let mut got = listed.listed_ips();
        got.sort();
        let mut want = expected;
        want.sort();
        assert_eq!(
            got, want,
            "case {:?}: expected pool {:?} to hold {want:?}, got {got:?}",
            case.case, case.destination
        );
    }
}

#[tokio::test]
async fn repeating_a_move_answers_the_same_way_and_changes_nothing_further() {
    let ses = Ses::new();
    let cases = move_cases();
    create_every_pool(&ses, &cases).await;
    for case in &cases {
        ses.allocate(case.ip, case.held_pool, case.warmup);
    }
    for case in &cases {
        let first = ses.put_in_pool(case.ip, case.destination).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, first.status, first.body
        );
    }
    let after_first = ses.stored();

    for case in &cases {
        let again = ses.put_in_pool(case.ip, case.destination).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected the repeated move to answer 200, got {} {:?}",
            case.case, again.status, again.body
        );
    }
    assert_eq!(
        ses.stored(),
        after_first,
        "case \"the store after every move was repeated\": expected {after_first:?}, got {:?}",
        ses.stored()
    );
}

#[tokio::test]
async fn an_address_the_account_does_not_hold_is_refused_as_a_missing_resource() {
    let ses = Ses::new();
    let created = ses.create_pool("marketing-pool").await;
    assert_eq!(
        created.status, 200,
        "case \"the destination pool\": expected 200, got {} {:?}",
        created.status, created.body
    );
    // One dedicated IP in the account, so a refusal cannot come from an empty table.
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    let before = ses.stored();

    // The rows differ along how near the request's address is to the one the account
    // does hold: a neighbour in the same /24, the same octets in another order, another
    // documentation range, a loopback, and the two edge addresses.
    let cases = [
        ("a neighbour of the held address", "198.51.100.2"),
        ("the recorded address", "198.51.100.77"),
        ("the same octets in another order", "100.51.198.1"),
        ("another documentation range", "192.0.2.42"),
        ("a loopback address", "127.0.0.1"),
        ("the all-zeroes address", "0.0.0.0"),
        ("the broadcast address", "255.255.255.255"),
    ];

    for (case, ip) in cases {
        let reply = ses.put_in_pool(ip, "marketing-pool").await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        // The refusal names the address the request asked about, taken from the request
        // rather than from anything stored.
        assert_eq!(
            reply.message(),
            format!("Could not find dedicated IP <{ip}> under this account."),
            "case {case:?}: expected the refusal to name {ip:?}, got {:?}",
            reply.message()
        );
    }

    // A refused move mints no address and moves none.
    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the refusals\": expected {before:?}, got {:?}",
        ses.stored()
    );
}

#[tokio::test]
async fn a_destination_pool_the_account_does_not_hold_is_refused_as_a_missing_resource() {
    let ses = Ses::new();
    let created = ses.create_pool("marketing-pool").await;
    assert_eq!(
        created.status, 200,
        "case \"the pool the addresses start in\": expected 200, got {} {:?}",
        created.status, created.body
    );
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    ses.allocate("192.0.2.8", None, WARMUP_COMPLETE_PERCENTAGE);
    let before = ses.stored();

    // The rows differ along the form of the pool name that does not exist and along
    // where the address being moved starts (in a pool, in none).
    let cases = [
        ("a pool never created", "198.51.100.1", "transactional-pool"),
        (
            "a name differing from the held pool only in case",
            "198.51.100.1",
            "Marketing-Pool",
        ),
        (
            "a name the held pool is a prefix of",
            "192.0.2.8",
            "marketing-pool-2",
        ),
        ("a one-character name", "192.0.2.8", "p"),
        ("a name carrying a space", "198.51.100.1", "marketing pool"),
    ];

    for (case, ip, pool) in cases {
        let reply = ses.put_in_pool(ip, pool).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("The requested pool <{pool}> does not exist."),
            "case {case:?}: expected the refusal to name {pool:?}, got {:?}",
            reply.message()
        );
    }

    // A refused move creates no pool and leaves every address where it was.
    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the refusals\": expected {before:?}, got {:?}",
        ses.stored()
    );
}

#[tokio::test]
async fn a_value_that_is_not_an_ipv4_address_is_refused_as_invalid_input() {
    let ses = Ses::new();
    let created = ses.create_pool("marketing-pool").await;
    assert_eq!(
        created.status, 200,
        "case \"the destination pool\": expected 200, got {} {:?}",
        created.status, created.body
    );
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    let before = ses.stored();

    // The rows differ along how the value fails to be the IPv4 address `Ip` documents:
    // a host name, a truncated quad, too many octets, an octet past 255, an octet that
    // is not a number at all, and an IPv6 address. The last row also names a pool the
    // account does not hold, so it pins that invalid input is the answer before either
    // resource is looked up.
    let cases = [
        ("a host name", "smtp.example.com", "marketing-pool"),
        ("three octets", "198.51.100", "marketing-pool"),
        ("five octets", "198.51.100.1.7", "marketing-pool"),
        ("an octet past 255", "198.51.100.256", "marketing-pool"),
        ("an octet that is not a number", "198.51.100.x", "marketing-pool"),
        ("an IPv6 address", "2001:db8::1", "no-such-pool"),
    ];

    for (case, ip, pool) in cases {
        let reply = ses.put_in_pool(ip, pool).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("Invalid IP address: {ip}. The value has to be an IPv4 address."),
            "case {case:?}: expected the refusal to name {ip:?}, got {:?}",
            reply.message()
        );
    }

    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the refusals\": expected {before:?}, got {:?}",
        ses.stored()
    );
}
