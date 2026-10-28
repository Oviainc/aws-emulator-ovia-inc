// SPDX-License-Identifier: CC0-1.0

//! Deleting a dedicated IP pool, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised. Dedicated IP addresses are written into the engine's own state, the
//! way the suites beside this one do: SES allocates an address out of band, so no
//! request here mints one.
//!
//! The rules these cases pin, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. `DeleteDedicatedIpPool` REMOVES the pool the request names from the account, with
//!    everything held on it (its scaling mode and its tags): the pool's own read and the
//!    tag listing of its ARN then answer `NotFoundException`, the listing no longer
//!    reports it, and every other pool the account holds is left exactly as it was;
//! 2. a dedicated IP address that was in the deleted pool stays in the account and is
//!    reported as being in no pool, rather than naming a pool the account no longer
//!    holds; an address of another pool keeps its pool;
//! 3. a pool the account does not hold is refused `NotFoundException` (404) — so the
//!    delete is not idempotent, and a second delete of the same name is refused too —
//!    and the refusal changes nothing.
//!
//! The tables span what those rules generalise over: the pool's name form, its scaling
//! mode, whether it carries tags, how many addresses are in it (several, one, none), and
//! every leg of the refusal.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, DedicatedIpRecord,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// `ListTagsForResource` binds `ResourceArn` to `@httpQuery`, and an ARN carries `:` and
// `/` — characters a query string gives its own meaning to — so the value is escaped the
// way an SDK escapes it, by the one escape this test binary shares.
use crate::contact_lists::query_escaped;

/// The account id every ARN this emulator mints carries, and the Region every call in
/// this suite is signed for, so the two an ARN it resolves has to carry.
const ACCOUNT_ID: &str = "000000000000";
const HOME: &str = "us-east-1";

/// `arn:aws:ses:<region>:<account>:dedicated-ip-pool/<name>`, the form SES writes a
/// dedicated IP pool of its own in.
fn pool_arn(pool_name: &str) -> String {
    format!("arn:aws:ses:{HOME}:{ACCOUNT_ID}:dedicated-ip-pool/{pool_name}")
}

/// One answered request: the status, the error shape when there was one, and the body.
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

    /// A member reached by its path through the nested structures, or `Null`.
    fn at(&self, path: &[&str]) -> Value {
        let mut here = &self.body;
        for step in path {
            match here.get(step) {
                Some(next) => here = next,
                None => return Value::Null,
            }
        }
        here.clone()
    }

    /// A string at `path`, or `""` when the member is absent.
    fn text_at(&self, path: &[&str]) -> String {
        self.at(path).as_str().unwrap_or_default().to_string()
    }

    /// The strings of the list member at `path`; an absent list reads as empty.
    fn strings_at(&self, path: &[&str]) -> BTreeSet<String> {
        self.at(path)
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| row.as_str().unwrap_or_default().to_string())
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
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"),
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

    async fn create_pool(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/dedicated-ip-pools", Some(body))
            .await
    }

    async fn get_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}"),
            None,
        )
        .await
    }

    /// `DeleteDedicatedIpPool`'s route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="DeleteDedicatedIpPool")`:
    /// `DELETE /v2/email/dedicated-ip-pools/{PoolName}`).
    async fn delete_pool(&self, pool_name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/dedicated-ip-pools/{pool_name}"),
            None,
        )
        .await
    }

    async fn list_pools(&self) -> Reply {
        self.call("GET", "/v2/email/dedicated-ip-pools", None).await
    }

    async fn get_dedicated_ip(&self, ip: &str) -> Reply {
        self.call("GET", &format!("/v2/email/dedicated-ips/{ip}"), None)
            .await
    }

    /// `ListTagsForResource` for `resource_arn`, which the operation binds to
    /// `@httpQuery`.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/tags?ResourceArn={}", query_escaped(resource_arn)),
            None,
        )
        .await
    }

    /// Put a dedicated IP in the account: no SES request allocates one, so a case that
    /// reads one back writes the record the allocation would have left.
    fn allocate(&self, ip: &str, pool_name: &str, warmup_percentage: i32) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().dedicated_ips.insert(
            ip.to_string(),
            DedicatedIpRecord::new(ip, Some(pool_name.to_string()), warmup_percentage),
        );
    }
}

/// One pool of the deletion table: how it is created, and which addresses are in it.
struct Row {
    case: &'static str,
    pool_name: &'static str,
    body: Value,
    scaling_mode: &'static str,
    /// The tags the create carries, as the pairs the tag listing has to report.
    tags: Vec<(&'static str, &'static str)>,
    /// The addresses in the pool, each with its stored warm-up percentage.
    addresses: Vec<(&'static str, i32)>,
}

/// The pools every case of this suite works over.
///
/// The rows differ along the name form (one character, both documented separators, a
/// long name), the `ScalingMode` (both the modes the enum declares, and a pool created
/// without one), whether the create carried tags, and how many addresses are in the pool
/// — two, one and none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pool with no ScalingMode holding two addresses",
            pool_name: "marketing-pool",
            body: json!({"PoolName": "marketing-pool"}),
            scaling_mode: "STANDARD",
            tags: Vec::new(),
            addresses: vec![("198.51.100.1", 0), ("198.51.100.2", 60)],
        },
        Row {
            case: "a STANDARD pool carrying tags and holding one address",
            pool_name: "transactional_pool",
            body: json!({
                "PoolName": "transactional_pool",
                "ScalingMode": "STANDARD",
                "Tags": [{"Key": "team", "Value": "growth"}, {"Key": "tier", "Value": "gold"}],
            }),
            scaling_mode: "STANDARD",
            tags: vec![("team", "growth"), ("tier", "gold")],
            addresses: vec![("192.0.2.8", 100)],
        },
        Row {
            case: "a MANAGED pool holding one address whose warm-up does not apply",
            pool_name: "managed-pool",
            body: json!({"PoolName": "managed-pool", "ScalingMode": "MANAGED"}),
            scaling_mode: "MANAGED",
            tags: Vec::new(),
            addresses: vec![("203.0.113.9", -1)],
        },
        Row {
            case: "a one-character pool name, holding no address",
            pool_name: "p",
            body: json!({"PoolName": "p"}),
            scaling_mode: "STANDARD",
            tags: Vec::new(),
            addresses: Vec::new(),
        },
        Row {
            case: "a long pool name carrying both documented separators",
            pool_name: "marketing_pool-for-eu-campaigns-2026",
            body: json!({
                "PoolName": "marketing_pool-for-eu-campaigns-2026",
                "ScalingMode": "MANAGED",
                "Tags": [{"Key": "owner", "Value": "eu"}],
            }),
            scaling_mode: "MANAGED",
            tags: vec![("owner", "eu")],
            addresses: vec![("192.0.2.77", 42)],
        },
    ]
}

/// Create every pool of the table and allocate the addresses that are in them.
async fn seeded() -> (Ses, Vec<Row>) {
    let ses = Ses::new();
    let rows = rows();
    for row in &rows {
        let reply = ses.create_pool(row.body.clone()).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (200, ""),
            "case {:?}: expected 200 from the create that sets the case up, got {} {:?}",
            row.case,
            reply.status,
            reply.body
        );
        for (ip, warmup) in &row.addresses {
            ses.allocate(ip, row.pool_name, *warmup);
        }
    }
    (ses, rows)
}

/// The tag pairs a tag listing reported, in the order reported.
fn reported_tags(reply: &Reply) -> Vec<(String, String)> {
    reply
        .at(&["Tags"])
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (
                        row["Key"].as_str().unwrap_or_default().to_string(),
                        row["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_deleted_pool_is_gone_and_every_other_pool_is_untouched() {
    let (ses, rows) = seeded().await;
    let mut remaining: Vec<&Row> = rows.iter().collect();

    while !remaining.is_empty() {
        let row = remaining.remove(0);
        let case = row.case;

        let deleted = ses.delete_pool(row.pool_name).await;
        assert_eq!(
            (deleted.status, deleted.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected 200 from the delete, got {} {:?}",
            deleted.status,
            deleted.body
        );

        // The pool's own read no longer reports it, and names the pool the request
        // asked about.
        let read = ses.get_pool(row.pool_name).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected the pool's read to answer 404 NotFoundException after the delete, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.message(),
            format!("The requested pool <{}> does not exist.", row.pool_name),
            "case {case:?}: expected the refusal to name {:?}, got {:?}",
            row.pool_name,
            read.message()
        );

        // Everything held on the pool went with it, so the tag listing of its ARN no
        // longer resolves a resource at all.
        let tags = ses.list_tags(&pool_arn(row.pool_name)).await;
        assert_eq!(
            (tags.status, tags.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected the tag listing of the deleted pool to answer 404 NotFoundException, got {} {:?}",
            tags.status,
            tags.body
        );

        // The listing reports exactly the pools that are left.
        let listed = ses.list_pools().await;
        let expected: BTreeSet<String> = remaining
            .iter()
            .map(|left| left.pool_name.to_string())
            .collect();
        assert_eq!(
            listed.strings_at(&["DedicatedIpPools"]),
            expected,
            "case {case:?}: expected the listing to report exactly {expected:?}, got {:?}",
            listed.strings_at(&["DedicatedIpPools"])
        );

        // An address that was in the deleted pool is still in the account, with its
        // warm-up untouched, and is in no pool rather than in one that is gone.
        for (ip, warmup) in &row.addresses {
            let address = ses.get_dedicated_ip(ip).await;
            assert_eq!(
                address.status, 200,
                "case {case:?}: expected the address {ip:?} to survive the delete, got {} {:?}",
                address.status, address.body
            );
            assert_eq!(
                (
                    address.at(&["DedicatedIp", "PoolName"]),
                    address.at(&["DedicatedIp", "WarmupPercentage"]),
                ),
                (Value::Null, Value::from(*warmup)),
                "case {case:?}: expected {ip:?} to report no pool and warm-up {warmup}, got {:?}",
                address.body
            );
        }

        // Every pool that is left still reads exactly as it was created, and its own
        // addresses still name it.
        for left in &remaining {
            let read = ses.get_pool(left.pool_name).await;
            assert_eq!(
                (
                    read.status,
                    read.text_at(&["DedicatedIpPool", "PoolName"]),
                    read.text_at(&["DedicatedIpPool", "ScalingMode"]),
                ),
                (
                    200,
                    left.pool_name.to_string(),
                    left.scaling_mode.to_string()
                ),
                "case {case:?}: deleting it must leave {:?} alone, got {} {:?}",
                left.pool_name,
                read.status,
                read.body
            );
            let tags = ses.list_tags(&pool_arn(left.pool_name)).await;
            let expected: Vec<(String, String)> = left
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                reported_tags(&tags),
                expected,
                "case {case:?}: deleting it must leave {:?}'s tags alone, expected {expected:?}, got {:?}",
                left.pool_name,
                reported_tags(&tags)
            );
            for (ip, _) in &left.addresses {
                let address = ses.get_dedicated_ip(ip).await;
                assert_eq!(
                    address.text_at(&["DedicatedIp", "PoolName"]),
                    left.pool_name,
                    "case {case:?}: expected {ip:?} to stay in {:?}, got {:?}",
                    left.pool_name,
                    address.body
                );
            }
        }
    }

    // Deleting every pool left the account holding none.
    let listed = ses.list_pools().await;
    assert_eq!(
        listed.strings_at(&["DedicatedIpPools"]),
        BTreeSet::new(),
        "case \"the listing after every pool was deleted\": expected no pool, got {:?}",
        listed.strings_at(&["DedicatedIpPools"])
    );
}

#[tokio::test]
async fn deleting_a_pool_the_account_does_not_hold_is_refused() {
    let (ses, rows) = seeded().await;
    // One pool is deleted first, so a second delete of that same name is one of the rows
    // below: the delete is not idempotent.
    let gone = rows[0].pool_name;
    let first = ses.delete_pool(gone).await;
    assert_eq!(
        first.status, 200,
        "case \"the delete that sets the second one up\": expected 200, got {} {:?}",
        first.status, first.body
    );

    let held: BTreeSet<String> = ses.list_pools().await.strings_at(&["DedicatedIpPools"]);

    // The rows differ along why the account does not hold the name: never created, the
    // same name in another case, a held name with a suffix, a name that is an address in
    // a pool, and a name the account held until a moment ago.
    let cases = [
        ("a pool that was never created", "no-such-pool".to_string()),
        (
            "the same name in another case",
            rows[1].pool_name.to_uppercase(),
        ),
        (
            "a held name with a suffix",
            format!("{}-2", rows[1].pool_name),
        ),
        ("a name that is one of the addresses", "192.0.2.8".to_string()),
        ("a pool deleted a moment ago", gone.to_string()),
    ];

    for (case, pool_name) in &cases {
        let reply = ses.delete_pool(pool_name).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        // The refusal names the pool the request asked about, taken from the request
        // rather than from anything stored.
        assert_eq!(
            reply.message(),
            format!("The requested pool <{pool_name}> does not exist."),
            "case {case:?}: expected the refusal to name {pool_name:?}, got {:?}",
            reply.message()
        );
    }

    // And the refusals left every pool the account does hold exactly where it was.
    let after: BTreeSet<String> = ses.list_pools().await.strings_at(&["DedicatedIpPools"]);
    assert_eq!(
        after, held,
        "case \"the listing after the refusals\": expected {held:?}, got {after:?}"
    );
    for row in &rows[1..] {
        let read = ses.get_pool(row.pool_name).await;
        assert_eq!(
            (
                read.status,
                read.text_at(&["DedicatedIpPool", "ScalingMode"])
            ),
            (200, row.scaling_mode.to_string()),
            "case \"{}: after the refusals\": expected it unchanged, got {} {:?}",
            row.pool_name,
            read.status,
            read.body
        );
    }
}
