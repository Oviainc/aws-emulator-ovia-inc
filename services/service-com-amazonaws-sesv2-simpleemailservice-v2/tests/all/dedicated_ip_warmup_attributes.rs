// SPDX-License-Identifier: CC0-1.0

//! Writing one dedicated IP address's warm-up attributes, and listing the addresses the
//! account holds, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and seeds the engine's own state where a dedicated IP has to be in the
//! account before a write can update it (SES allocates one out of band, so no request
//! mints one).
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded request:
//!
//! 1. `PutDedicatedIpWarmupAttributes` PERSISTS the warm-up percentage the request
//!    carries onto the account's record of the address the request names, and the
//!    address's own read (`GetDedicatedIp`) and the listing (`GetDedicatedIps`) then
//!    report that percentage and the warm-up status derived from it — leaving every other
//!    address exactly as it was, and answering the same way when the same write is
//!    repeated;
//! 2. a `WarmupPercentage` outside the range `DedicatedIp$WarmupPercentage` documents for
//!    a standard dedicated IP (`0`..=`100`) is REFUSED (`BadRequestException`, 400) and
//!    persists nothing — including the one value outside that range the member documents,
//!    which is what SES reports for an address whose warm-up it handles itself, never a
//!    progress a caller sets — and that refusal comes before the address is looked up, so
//!    it is the answer even for an address the account does not hold;
//! 3. an address the account holds no dedicated IP for is refused `NotFoundException`
//!    (404) and mints nothing, and a value that is not an IPv4 address at all is refused
//!    `BadRequestException` (400);
//! 4. `GetDedicatedIps` reports the addresses the account holds in one stable order, a
//!    walk with ANY `PageSize` visits each address exactly once — no duplicate and no drop
//!    — and terminates; naming a pool narrows the listing to that pool's addresses, a pool
//!    the account does not hold is refused `NotFoundException` (404), and a `NextToken`
//!    the caller did not get from this listing is refused `BadRequestException` (400).

use std::collections::BTreeSet;
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

    /// The `DedicatedIps` a listing reports; an absent list reads as empty.
    fn dedicated_ips(&self) -> Vec<Value> {
        self.body
            .get("DedicatedIps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `NextToken` a listing carried, or `None` when it carried none.
    fn next_token(&self) -> Option<String> {
        self.body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
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
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
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

    /// `PutDedicatedIpWarmupAttributes`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="PutDedicatedIpWarmupAttributes")`:
    /// `PUT /v2/email/dedicated-ips/{Ip}/warmup`).
    async fn put_warmup(&self, ip: &str, warmup_percentage: i32) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/dedicated-ips/{ip}/warmup"),
            Some(json!({ "WarmupPercentage": warmup_percentage })),
        )
        .await
    }

    /// `GetDedicatedIp`'s route: `GET /v2/email/dedicated-ips/{Ip}`.
    async fn get_dedicated_ip(&self, ip: &str) -> Reply {
        self.call("GET", &format!("/v2/email/dedicated-ips/{ip}"), None)
            .await
    }

    /// `GetDedicatedIps`'s route: `GET /v2/email/dedicated-ips`, whose `PoolName`,
    /// `PageSize` and `NextToken` are all bound to the query string.
    async fn get_dedicated_ips(
        &self,
        pool_name: Option<&str>,
        page_size: Option<i32>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(pool_name) = pool_name {
            query.push(format!("PoolName={pool_name}"));
        }
        if let Some(page_size) = page_size {
            query.push(format!("PageSize={page_size}"));
        }
        if let Some(next_token) = next_token {
            // A token is base64, whose `+`, `/` and `=` the query string gives its own
            // meaning to, so it travels escaped the way an SDK escapes it.
            query.push(format!("NextToken={}", crate::contact_lists::query_escaped(next_token)));
        }
        let path = if query.is_empty() {
            "/v2/email/dedicated-ips".to_string()
        } else {
            format!("/v2/email/dedicated-ips?{}", query.join("&"))
        };
        self.call("GET", &path, None).await
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
    /// updates one writes the record the allocation would have left.
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

/// One row of the write table: an address the account holds, the warm-up it was holding,
/// the warm-up the request sets, and the status a read then reports.
struct WriteCase {
    case: &'static str,
    ip: &'static str,
    pool_name: Option<&'static str>,
    held: i32,
    requested: i32,
    warmup_status: &'static str,
}

/// Every row of the write table: the rows differ along the percentage written (both
/// published bounds, one short of each, a value between them), the percentage the address
/// was holding (including the value that does not apply, which SES reports for an address
/// in a managed pool), whether the warm-up moves up or back down, the pool membership (a
/// pool, no pool) and the form of the address.
fn write_cases() -> Vec<WriteCase> {
    vec![
        WriteCase {
            case: "a finished warm-up restarted from the low bound",
            ip: "198.51.100.1",
            pool_name: Some("marketing-pool"),
            held: WARMUP_COMPLETE_PERCENTAGE,
            requested: WARMUP_START_PERCENTAGE,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "a warm-up resumed part way",
            ip: "198.51.100.23",
            pool_name: Some("transactional_pool"),
            held: WARMUP_START_PERCENTAGE,
            requested: 37,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "a warm-up moved to one short of complete",
            ip: "198.51.100.254",
            pool_name: Some("p"),
            held: 10,
            requested: WARMUP_COMPLETE_PERCENTAGE - 1,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "a warm-up moved to one past the low bound",
            ip: "192.0.2.8",
            pool_name: Some("edge-pool"),
            held: 60,
            requested: WARMUP_START_PERCENTAGE + 1,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "a warm-up declared complete, on an address in no pool",
            ip: "10.0.0.1",
            pool_name: None,
            held: 42,
            requested: WARMUP_COMPLETE_PERCENTAGE,
            warmup_status: "DONE",
        },
        WriteCase {
            case: "a warm-up written to the value the address already held",
            ip: "172.16.255.3",
            pool_name: None,
            held: 50,
            requested: 50,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "an address whose warm-up did not apply until now",
            ip: "203.0.113.9",
            pool_name: Some("managed-pool"),
            held: MANAGED_WARMUP_PERCENTAGE,
            requested: 25,
            warmup_status: "IN_PROGRESS",
        },
        WriteCase {
            case: "the highest address form",
            ip: "255.255.255.255",
            pool_name: Some("warmed-pool"),
            held: 1,
            requested: WARMUP_COMPLETE_PERCENTAGE,
            warmup_status: "DONE",
        },
    ]
}

/// The `DedicatedIp` document a read of `case` must answer with once the write has
/// landed.
fn expected_document(case: &WriteCase) -> Value {
    let mut members = serde_json::Map::new();
    members.insert("Ip".to_string(), Value::from(case.ip));
    members.insert("WarmupStatus".to_string(), Value::from(case.warmup_status));
    members.insert("WarmupPercentage".to_string(), Value::from(case.requested));
    if let Some(pool) = case.pool_name {
        members.insert("PoolName".to_string(), Value::from(pool));
    }
    Value::Object(members)
}

#[tokio::test]
async fn a_write_persists_the_warm_up_of_the_address_it_names() {
    let ses = Ses::new();
    let cases = write_cases();
    // Every address is in the account at once, so each write has to find the record the
    // request names rather than the only one there is.
    for case in &cases {
        ses.allocate(case.ip, case.pool_name, case.held);
    }

    for case in &cases {
        let written = ses.put_warmup(case.ip, case.requested).await;
        assert_eq!(
            written.status, 200,
            "case {:?}: expected 200, got {} {} {:?}",
            case.case,
            written.status,
            written.error_code(),
            written.body
        );

        // Read back through the address's own read: the stored percentage, and the status
        // derived from it.
        let read = ses.get_dedicated_ip(case.ip).await;
        assert_eq!(
            read.dedicated_ip(),
            expected_document(case),
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            expected_document(case),
            read.dedicated_ip()
        );

        // And repeating the very same write answers the same way and reports the same
        // record: the write is the percentage it names, not a step along from it.
        let again = ses.put_warmup(case.ip, case.requested).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected the repeated write to answer 200, got {} {:?}",
            case.case, again.status, again.body
        );
        let read_again = ses.get_dedicated_ip(case.ip).await;
        assert_eq!(
            read_again.dedicated_ip(),
            expected_document(case),
            "case {:?}: expected the repeated write to leave {:?}, got {:?}",
            case.case,
            expected_document(case),
            read_again.dedicated_ip()
        );
    }

    // The listing reports every address with the warm-up that was written to it, so the
    // two readers cannot describe one address differently.
    let listed = ses.get_dedicated_ips(None, None, None).await;
    let mut expected: Vec<Value> = cases.iter().map(expected_document).collect();
    expected.sort_by_key(|row| row["Ip"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        listed.dedicated_ips(),
        expected,
        "case \"the listing after the writes\": expected {expected:?}, got {:?}",
        listed.dedicated_ips()
    );
}

#[tokio::test]
async fn a_write_leaves_every_other_address_as_it_was() {
    let ses = Ses::new();
    let cases = write_cases();
    for case in &cases {
        ses.allocate(case.ip, case.pool_name, case.held);
    }

    // One write at a time, each checked against the whole table: only the address the
    // request named may have moved.
    for (index, case) in cases.iter().enumerate() {
        let reply = ses.put_warmup(case.ip, case.requested).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );

        let mut expected: Vec<(String, Option<String>, i32)> = cases
            .iter()
            .enumerate()
            .map(|(other, row)| {
                let percentage = if other <= index { row.requested } else { row.held };
                (
                    row.ip.to_string(),
                    row.pool_name.map(str::to_string),
                    percentage,
                )
            })
            .collect();
        expected.sort();
        assert_eq!(
            ses.stored(),
            expected,
            "case {:?}: expected the store to be {expected:?}, got {:?}",
            case.case,
            ses.stored()
        );
    }
}

#[tokio::test]
async fn a_warm_up_percentage_outside_the_documented_range_is_refused() {
    let ses = Ses::new();
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    let before = ses.stored();

    // The rows differ along how the number falls outside the `0`-`100` range
    // `DedicatedIp$WarmupPercentage` documents for a standard dedicated IP: the one value
    // outside it the member itself documents (what SES reports for an address in a
    // managed pool), further below it, one past the top, far past it, and each end of the
    // integer the member is modelled as.
    let cases = [
        ("the value that does not apply", MANAGED_WARMUP_PERCENTAGE),
        ("one below that", MANAGED_WARMUP_PERCENTAGE - 1),
        ("one below the low bound", WARMUP_START_PERCENTAGE - 1),
        ("well below the low bound", -100),
        ("one past the top bound", WARMUP_COMPLETE_PERCENTAGE + 1),
        ("far past the top bound", 1_000),
        ("the largest integer", i32::MAX),
        ("the smallest integer", i32::MIN),
    ];
    let expected_message = format!(
        "Warmup Percentage must be between {WARMUP_START_PERCENTAGE} and {WARMUP_COMPLETE_PERCENTAGE}."
    );

    for (case, percentage) in cases {
        let reply = ses.put_warmup("198.51.100.1", percentage).await;
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
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            reply.message()
        );

        // The percentage is refused whatever the account holds, so an address it holds no
        // dedicated IP for is answered the same way rather than as a missing resource.
        let unheld = ses.put_warmup("198.51.100.200", percentage).await;
        assert_eq!(
            (unheld.status, unheld.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?} on an address the account does not hold: expected 400 BadRequestException, got {} {} {:?}",
            unheld.status,
            unheld.error_code(),
            unheld.body
        );
    }

    // And every refusal persisted nothing: the held address kept its warm-up and no
    // record was minted.
    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the refusals\": expected {before:?}, got {:?}",
        ses.stored()
    );
}

#[tokio::test]
async fn an_address_the_account_does_not_hold_is_refused_as_a_missing_resource() {
    let ses = Ses::new();
    // One dedicated IP in the account, so a refusal cannot come from an empty table.
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    let before = ses.stored();

    // The rows differ along how near the request's address is to the one the account does
    // hold: a neighbour in the same /24, the same octets in another order, another
    // documentation range, a loopback, the all-zeroes address and the broadcast address.
    let cases = [
        ("a neighbour of the held address", "198.51.100.2"),
        ("the same octets in another order", "100.51.198.1"),
        ("another documentation range", "192.0.2.42"),
        ("a loopback address", "127.0.0.1"),
        ("the all-zeroes address", "0.0.0.0"),
        ("the broadcast address", "255.255.255.255"),
    ];

    for (case, ip) in cases {
        let reply = ses.put_warmup(ip, 50).await;
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

    // A write to an address the account does not hold mints no dedicated IP.
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
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);
    let before = ses.stored();

    // The rows differ along how the value fails to be the IPv4 address `Ip` documents: a
    // host name, a truncated quad, too many octets, an octet past 255, an octet that is
    // not a number at all, and an IPv6 address.
    let cases = [
        ("a host name", "smtp.example.com"),
        ("three octets", "198.51.100"),
        ("five octets", "198.51.100.1.7"),
        ("an octet past 255", "198.51.100.256"),
        ("an octet that is not a number", "198.51.100.x"),
        ("an IPv6 address", "2001:db8::1"),
    ];

    for (case, ip) in cases {
        let reply = ses.put_warmup(ip, 50).await;
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

#[tokio::test]
async fn a_walk_of_the_listing_visits_every_address_exactly_once() {
    let ses = Ses::new();
    let cases = write_cases();
    for case in &cases {
        ses.allocate(case.ip, case.pool_name, case.held);
    }
    let held: BTreeSet<String> = cases.iter().map(|case| case.ip.to_string()).collect();

    // The rows differ along the page size: one address at a time, a size that does not
    // divide the table, one that is exactly the table, one larger than it, and the
    // non-positive sizes that name no truncation at all.
    for page_size in [None, Some(0), Some(-1), Some(1), Some(3), Some(8), Some(50)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let reply = ses
                .get_dedicated_ips(None, page_size, token.as_deref())
                .await;
            assert_eq!(
                reply.status, 200,
                "case \"page size {page_size:?}\": expected 200, got {} {:?}",
                reply.status, reply.body
            );
            for row in reply.dedicated_ips() {
                seen.push(row["Ip"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= held.len() + 1,
                "case \"page size {page_size:?}\": expected the walk to terminate within {} pages, got {pages}",
                held.len() + 1
            );
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case \"page size {page_size:?}\": expected no address twice, got {seen:?}"
        );
        assert_eq!(
            sorted,
            held.iter().cloned().collect::<Vec<String>>(),
            "case \"page size {page_size:?}\": expected every held address, got {sorted:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_narrowed_to_a_pool_reports_that_pools_addresses() {
    let ses = Ses::new();
    // The pools the addresses are in have to exist before the listing can be narrowed to
    // one, so they are created the way a caller creates them.
    for pool in ["marketing-pool", "transactional_pool", "empty-pool"] {
        let created = ses.create_pool(pool).await;
        assert_eq!(
            created.status, 200,
            "case \"create {pool:?}\": expected 200, got {} {:?}",
            created.status, created.body
        );
    }
    ses.allocate("198.51.100.1", Some("marketing-pool"), 10);
    ses.allocate("198.51.100.2", Some("marketing-pool"), 20);
    ses.allocate("192.0.2.8", Some("transactional_pool"), 30);
    ses.allocate("10.0.0.1", None, 40);

    // The rows differ along what the named pool holds: two addresses, one address, and a
    // pool the account holds that no address is in.
    let cases = [
        (
            "a pool holding two addresses",
            "marketing-pool",
            vec!["198.51.100.1", "198.51.100.2"],
        ),
        (
            "a pool holding one address",
            "transactional_pool",
            vec!["192.0.2.8"],
        ),
        ("a pool holding none", "empty-pool", vec![]),
    ];

    for (case, pool, expected) in cases {
        let reply = ses.get_dedicated_ips(Some(pool), None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        let listed: Vec<String> = reply
            .dedicated_ips()
            .iter()
            .map(|row| row["Ip"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // A pool the account does not hold is a resource that does not exist, not an empty
    // listing.
    for (case, pool) in [
        ("a pool that was never created", "no-such-pool"),
        ("a pool name that is one of the addresses", "198.51.100.1"),
    ] {
        let reply = ses.get_dedicated_ips(Some(pool), None, None).await;
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
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_hand_out_is_refused() {
    let ses = Ses::new();
    ses.allocate("198.51.100.1", Some("marketing-pool"), 10);
    ses.allocate("198.51.100.2", Some("marketing-pool"), 20);

    // The rows differ along where the token came from: a made-up string, valid base64 of
    // something that is not a position in any listing, a token tagged for another
    // listing, and an empty value.
    let other_listing = aws_smithy_types::base64::encode("sesv2:dedicated-ip-pools:a-pool");
    let not_a_position = aws_smithy_types::base64::encode("198.51.100.1");
    let cases = [
        ("a made-up string", "not-a-token".to_string()),
        ("base64 of something else", not_a_position),
        ("a token of another listing", other_listing),
        ("an empty token", String::new()),
    ];

    for (case, token) in cases {
        let reply = ses.get_dedicated_ips(None, None, Some(&token)).await;
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
            "Invalid next token.",
            "case {case:?}: expected \"Invalid next token.\", got {:?}",
            reply.message()
        );
    }
}
