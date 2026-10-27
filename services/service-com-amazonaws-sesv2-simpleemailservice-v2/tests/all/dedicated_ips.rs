// SPDX-License-Identifier: CC0-1.0

//! Reading one dedicated IP address, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reads the engine's own state where a dedicated IP has to be in
//! the account before a read can report it (SES allocates one out of band, so no
//! request mints one).
//!
//! The rules these cases pin:
//!
//! * `GetDedicatedIp` REPORTS the account's record of the address the request names —
//!   the address itself, the pool it is in and its warm-up — and derives the warm-up
//!   status from the stored percentage: a percentage that does not apply is
//!   `NOT_APPLICABLE`, a fully warmed-up address is `DONE`, and anything below that is
//!   `IN_PROGRESS`;
//! * reading changes nothing, so a second read of the same address, and a read of
//!   every other address, answer exactly as the first did;
//! * an address the account holds no dedicated IP for is refused with
//!   `NotFoundException` (404), and a value that is not an IPv4 address at all with
//!   `BadRequestException` (400).
//!
//! The tables below span what the rules generalise over: the form of the address, the
//! pool it is in (a pool, no pool), the warm-up percentage (both published bounds, a
//! value between them, one short of complete, and the value that does not apply), and
//! every leg of each refusal.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, DedicatedIpRecord, MANAGED_WARMUP_PERCENTAGE,
    WARMUP_COMPLETE_PERCENTAGE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
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

    /// The `DedicatedIp` the response reports, or `Value::Null` when it reports none.
    fn dedicated_ip(&self) -> Value {
        self.body
            .get("DedicatedIp")
            .cloned()
            .unwrap_or(Value::Null)
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

    /// `GetDedicatedIp`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="GetDedicatedIp")`:
    /// `GET /v2/email/dedicated-ips/{Ip}`).
    async fn get_dedicated_ip(&self, ip: &str) -> Reply {
        let region = "us-east-1";
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}/v2/email/dedicated-ips/{ip}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(bytes::Bytes::new())
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

    /// Put a dedicated IP in the account: no SES request allocates one, so a case that
    /// reads one back writes the record the allocation would have left.
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
    /// about a read not changing anything compares against.
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

/// One row of the report table: a dedicated IP the account holds, and the warm-up
/// status a read of it must report.
struct ReportCase {
    case: &'static str,
    ip: &'static str,
    pool_name: Option<&'static str>,
    warmup_percentage: i32,
    warmup_status: &'static str,
}

/// Every row of the report table: one dedicated IP per row, spanning the address form,
/// pool membership and the warm-up percentages `DedicatedIp$WarmupPercentage`
/// documents — `0`..=`100` for a standard address and `-1` where it does not apply.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "warm-up not started, in a pool",
            ip: "198.51.100.1",
            pool_name: Some("marketing-pool"),
            warmup_percentage: 0,
            warmup_status: "IN_PROGRESS",
        },
        ReportCase {
            case: "warm-up part way, in a pool whose name carries an underscore",
            ip: "198.51.100.23",
            pool_name: Some("transactional_pool"),
            warmup_percentage: 37,
            warmup_status: "IN_PROGRESS",
        },
        ReportCase {
            case: "warm-up one short of complete",
            ip: "198.51.100.254",
            pool_name: Some("p"),
            warmup_percentage: WARMUP_COMPLETE_PERCENTAGE - 1,
            warmup_status: "IN_PROGRESS",
        },
        ReportCase {
            case: "warm-up complete, in a pool",
            ip: "192.0.2.8",
            pool_name: Some("warmed-pool"),
            warmup_percentage: WARMUP_COMPLETE_PERCENTAGE,
            warmup_status: "DONE",
        },
        ReportCase {
            case: "warm-up complete, in no pool at all",
            ip: "10.0.0.1",
            pool_name: None,
            warmup_percentage: WARMUP_COMPLETE_PERCENTAGE,
            warmup_status: "DONE",
        },
        ReportCase {
            case: "warm-up not started, in no pool at all",
            ip: "172.16.255.3",
            pool_name: None,
            warmup_percentage: 0,
            warmup_status: "IN_PROGRESS",
        },
        ReportCase {
            case: "in a managed pool, where the percentage does not apply",
            ip: "203.0.113.9",
            pool_name: Some("managed-pool"),
            warmup_percentage: MANAGED_WARMUP_PERCENTAGE,
            warmup_status: "NOT_APPLICABLE",
        },
        ReportCase {
            case: "the highest address form",
            ip: "255.255.255.255",
            pool_name: Some("edge-pool"),
            warmup_percentage: 50,
            warmup_status: "IN_PROGRESS",
        },
    ]
}

/// What a read of `case` must answer with, as the wire document.
fn expected_document(case: &ReportCase) -> Value {
    let mut members = serde_json::Map::new();
    members.insert("Ip".to_string(), Value::from(case.ip));
    members.insert(
        "WarmupStatus".to_string(),
        Value::from(case.warmup_status),
    );
    members.insert(
        "WarmupPercentage".to_string(),
        Value::from(case.warmup_percentage),
    );
    if let Some(pool) = case.pool_name {
        members.insert("PoolName".to_string(), Value::from(pool));
    }
    Value::Object(members)
}

#[tokio::test]
async fn a_read_reports_the_accounts_record_of_the_address_it_names() {
    let ses = Ses::new();
    let cases = report_cases();
    // Every address is in the account at once, so each read has to pick out the record
    // the request names rather than the only one there is.
    for case in &cases {
        ses.allocate(case.ip, case.pool_name, case.warmup_percentage);
    }

    for case in &cases {
        let reply = ses.get_dedicated_ip(case.ip).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );
        assert_eq!(
            reply.dedicated_ip(),
            expected_document(case),
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            expected_document(case),
            reply.dedicated_ip()
        );
    }
}

#[tokio::test]
async fn reading_an_address_changes_nothing_a_later_caller_sees() {
    let ses = Ses::new();
    let cases = report_cases();
    for case in &cases {
        ses.allocate(case.ip, case.pool_name, case.warmup_percentage);
    }
    let before = ses.stored();

    // Read every address twice, plus one the account does not hold: a report must not
    // move a warm-up along, move an address between pools, or add or drop a record.
    for case in &cases {
        let first = ses.get_dedicated_ip(case.ip).await;
        let second = ses.get_dedicated_ip(case.ip).await;
        assert_eq!(
            first.dedicated_ip(),
            second.dedicated_ip(),
            "case {:?}: expected the second read to answer as the first did ({:?}), got {:?}",
            case.case,
            first.dedicated_ip(),
            second.dedicated_ip()
        );
    }
    let missed = ses.get_dedicated_ip("198.51.100.77").await;
    assert_eq!(
        missed.status, 404,
        "case \"a read of an address the account does not hold\": expected 404, got {} {:?}",
        missed.status, missed.body
    );

    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the reads\": expected {before:?}, got {:?}",
        ses.stored()
    );
}

#[tokio::test]
async fn an_address_the_account_does_not_hold_is_refused_as_a_missing_resource() {
    let ses = Ses::new();
    // One dedicated IP in the account, so a refusal cannot come from an empty table.
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);

    // The rows differ along how near the request's address is to the one the account
    // does hold: a neighbour in the same /24, another documentation range, a loopback
    // and a broadcast address.
    let cases = [
        ("a neighbour of the held address", "198.51.100.2"),
        ("the same octets in another order", "100.51.198.1"),
        ("another documentation range", "192.0.2.42"),
        ("a loopback address", "127.0.0.1"),
        ("the all-zeroes address", "0.0.0.0"),
        ("the broadcast address", "255.255.255.255"),
    ];

    for (case, ip) in cases {
        let reply = ses.get_dedicated_ip(ip).await;
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
}

#[tokio::test]
async fn a_value_that_is_not_an_ipv4_address_is_refused_as_invalid_input() {
    let ses = Ses::new();
    ses.allocate("198.51.100.1", Some("marketing-pool"), 25);

    // The rows differ along how the value fails to be the IPv4 address `Ip` documents:
    // a host name, a truncated quad, too many octets, an octet past 255, an octet that
    // is not a number at all, and an IPv6 address.
    let cases = [
        ("a host name", "smtp.example.com"),
        ("three octets", "198.51.100"),
        ("five octets", "198.51.100.1.7"),
        ("an octet past 255", "198.51.100.256"),
        ("an octet that is not a number", "198.51.100.x"),
        ("an IPv6 address", "2001:db8::1"),
    ];

    for (case, ip) in cases {
        let reply = ses.get_dedicated_ip(ip).await;
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

    // And the refusals left the account's one dedicated IP exactly as it was.
    assert_eq!(
        ses.stored(),
        vec![(
            "198.51.100.1".to_string(),
            Some("marketing-pool".to_string()),
            25
        )],
        "case \"the store after the refusals\": expected the held address untouched, got {:?}",
        ses.stored()
    );
}
