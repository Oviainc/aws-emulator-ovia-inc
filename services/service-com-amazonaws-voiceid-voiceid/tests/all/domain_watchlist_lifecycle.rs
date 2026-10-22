// SPDX-License-Identifier: CC0-1.0

//! The domain + watchlist lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the router the production emulator mounts
//! (`awsJson1_0`, `POST /`, `X-Amz-Target: VoiceID.<Operation>`), so what is
//! checked is the HTTP behaviour a client sees, and every read is served from
//! what an earlier request persisted.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_voiceid_voiceid::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the ARNs the service mints must carry it.
const REGION: &str = "us-west-2";

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Length of a `DomainId`/`WatchlistId`: the model pins both at `^[a-zA-Z0-9]{22}$`.
const ID_LEN: usize = 22;

/// A well-formed `DomainId` that names no domain — 22 alphanumeric characters, so
/// it passes the modelled constraint and can only fail on lookup.
const ABSENT_DOMAIN_ID: &str = "ZZZZZZZZZZZZZZZZZZZZZZ";

/// One emulator plus the calls this suite makes against it.
struct Emulator {
    router: MultiServiceRouter,
}

/// One response: the status and the parsed JSON body (plus the raw body, for the
/// boundary refusals that render their code outside the modelled shape).
struct Reply {
    status: u16,
    body: Value,
    raw: String,
}

impl Reply {
    /// The wire error code (`__type`), or the whole body when the envelope
    /// carries none.
    fn error_code(&self) -> String {
        match self.body.get("__type").and_then(Value::as_str) {
            Some(code) => code.to_string(),
            None => self.raw.clone(),
        }
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises service behaviour, not SigV4; the credential scope
        // on each request is still what routes it and names its region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, target: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://voiceid.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("VoiceID.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/voiceid/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let raw = String::from_utf8_lossy(&bytes).to_string();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply { status, body, raw }
    }

    /// Create a domain and return its `Domain`, failing the test on any refusal.
    async fn create_domain(&self, request: &Value) -> Value {
        let reply = self.call("CreateDomain", request).await;
        assert_eq!(
            reply.status, 200,
            "case {request}: CreateDomain must succeed, got {} {}",
            reply.status, reply.raw
        );
        reply.body["Domain"].clone()
    }
}

/// Whether `value` is a `^[a-zA-Z0-9]{22}$` resource id.
fn is_resource_id(value: &str) -> bool {
    value.len() == ID_LEN && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The ARN a domain of `domain_id` must carry, per the model's
/// `^arn:aws(-[^:]+)?:voiceid.+:[0-9]{12}:domain/[a-zA-Z0-9]{22}$`.
fn expected_domain_arn(domain_id: &str) -> String {
    format!("arn:aws:voiceid:{REGION}:{ACCOUNT}:domain/{domain_id}")
}

/// A string member of a JSON object, as `&str`.
fn string_at<'a>(value: &'a Value, member: &str) -> &'a str {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("member {member} must be a string in {value}"))
}

/// The CreateDomain requests this suite spans: the rows differ along what the
/// rule generalises over — description present or absent, tag count, name shape,
/// and the form of the KMS key identifier.
fn create_domain_rows() -> Vec<Value> {
    vec![
        json!({
            "Name": "bootstrap-domain",
            "Description": "demo",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }),
        // No Description, no Tags: both members are optional, so neither may be
        // invented on the way out.
        json!({
            "Name": "D",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/my-voiceid-key"}
        }),
        json!({
            "Name": "domain_with_underscores-and-9-digits",
            "Description": "A longer description with punctuation: a/b=c+d-e%f@g",
            "ServerSideEncryptionConfiguration": {
                "KmsKeyId": "12345678-1234-1234-1234-123456789012"
            },
            "Tags": [{"Key": "team", "Value": "voice"}]
        }),
        json!({
            "Name": "X9",
            "Description": "two tags",
            "ServerSideEncryptionConfiguration": {
                "KmsKeyId": "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012"
            },
            "Tags": [
                {"Key": "team", "Value": "voice"},
                {"Key": "env", "Value": ""}
            ]
        }),
        // The upper bound of DomainName (length 1..256).
        json!({
            "Name": "n".repeat(256),
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }),
    ]
}

#[tokio::test]
async fn a_created_domain_is_read_back_identically_by_describe_and_list() {
    let emulator = Emulator::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for request in create_domain_rows() {
        let created = emulator.create_domain(&request).await;

        let domain_id = string_at(&created, "DomainId").to_string();
        assert!(
            is_resource_id(&domain_id),
            "case {request}: DomainId must match ^[a-zA-Z0-9]{{22}}$, got {domain_id:?}"
        );
        assert!(
            seen_ids.insert(domain_id.clone()),
            "case {request}: DomainId {domain_id} was minted twice"
        );
        assert_eq!(
            string_at(&created, "Arn"),
            expected_domain_arn(&domain_id),
            "case {request}: Arn must name the domain in the signed region, got {created}"
        );
        assert_eq!(
            string_at(&created, "DomainStatus"),
            "ACTIVE",
            "case {request}: a created domain is ACTIVE, got {created}"
        );
        assert_eq!(
            created["Name"], request["Name"],
            "case {request}: Name must be the stored name, got {created}"
        );
        assert_eq!(
            created["Description"],
            request.get("Description").cloned().unwrap_or(Value::Null),
            "case {request}: Description must be the stored description, got {created}"
        );
        assert_eq!(
            created["ServerSideEncryptionConfiguration"]["KmsKeyId"],
            request["ServerSideEncryptionConfiguration"]["KmsKeyId"],
            "case {request}: the stored KMS key must be reported, got {created}"
        );
        assert!(
            created["CreatedAt"].is_number() && created["UpdatedAt"].is_number(),
            "case {request}: CreatedAt/UpdatedAt must be timestamps, got {created}"
        );
        assert!(
            is_resource_id(string_at(&created["WatchlistDetails"], "DefaultWatchlistId")),
            "case {request}: WatchlistDetails.DefaultWatchlistId must be a watchlist id, got {created}"
        );

        // Read back through DescribeDomain: the same record, not a rebuilt one.
        let described = emulator
            .call("DescribeDomain", &json!({"DomainId": domain_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {request}: DescribeDomain must succeed, got {} {}",
            described.status, described.raw
        );
        assert_eq!(
            described.body["Domain"], created,
            "case {request}: DescribeDomain must return the stored domain, got {}",
            described.raw
        );

        // And through ListDomains, whose summaries carry the same stored fields.
        let listed = emulator.call("ListDomains", &json!({"MaxResults": 10})).await;
        assert_eq!(
            listed.status, 200,
            "case {request}: ListDomains must succeed, got {} {}",
            listed.status, listed.raw
        );
        let summary = listed.body["DomainSummaries"]
            .as_array()
            .expect("DomainSummaries is a list")
            .iter()
            .find(|summary| summary["DomainId"] == created["DomainId"])
            .cloned()
            .unwrap_or_else(|| panic!("case {request}: the domain must be listed, got {}", listed.raw));
        assert_eq!(
            summary, created,
            "case {request}: the summary must carry the stored domain's fields, got {}",
            listed.raw
        );
    }
}

#[tokio::test]
async fn every_domain_owns_a_default_watchlist_and_the_watchlists_created_in_it() {
    let emulator = Emulator::new();

    // Rows differ in how many extra watchlists the domain gets and in the
    // watchlist members supplied.
    let rows: Vec<Vec<Value>> = vec![
        vec![],
        vec![json!({"Name": "vip-list"})],
        vec![
            json!({"Name": "vip-list", "Description": "VIPs"}),
            json!({"Name": "W"}),
            json!({"Name": "list_3-with-digits-9"}),
        ],
    ];

    for extra in rows {
        let created_domain = emulator
            .create_domain(&json!({
                "Name": "watchlist-host",
                "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
            }))
            .await;
        let domain_id = string_at(&created_domain, "DomainId").to_string();
        let default_watchlist_id =
            string_at(&created_domain["WatchlistDetails"], "DefaultWatchlistId").to_string();

        let mut expected_ids = vec![default_watchlist_id.clone()];
        for request in &extra {
            let mut request = request.clone();
            request["DomainId"] = json!(domain_id);
            let reply = emulator.call("CreateWatchlist", &request).await;
            assert_eq!(
                reply.status, 200,
                "case {request}: CreateWatchlist must succeed, got {} {}",
                reply.status, reply.raw
            );
            let watchlist = reply.body["Watchlist"].clone();
            let watchlist_id = string_at(&watchlist, "WatchlistId").to_string();
            assert!(
                is_resource_id(&watchlist_id),
                "case {request}: WatchlistId must match ^[a-zA-Z0-9]{{22}}$, got {watchlist_id:?}"
            );
            assert_eq!(
                watchlist["DefaultWatchlist"],
                json!(false),
                "case {request}: a watchlist created by a caller is not the default one, got {watchlist}"
            );
            assert_eq!(
                watchlist["DomainId"],
                json!(domain_id),
                "case {request}: the watchlist must name its domain, got {watchlist}"
            );
            assert_eq!(
                watchlist["Name"], request["Name"],
                "case {request}: Name must be the stored name, got {watchlist}"
            );
            assert_eq!(
                watchlist["Description"],
                request.get("Description").cloned().unwrap_or(Value::Null),
                "case {request}: Description must be the stored description, got {watchlist}"
            );

            // Round trip: DescribeWatchlist serves the same stored record.
            let described = emulator
                .call(
                    "DescribeWatchlist",
                    &json!({"DomainId": domain_id, "WatchlistId": watchlist_id}),
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {request}: DescribeWatchlist must succeed, got {} {}",
                described.status, described.raw
            );
            assert_eq!(
                described.body["Watchlist"], watchlist,
                "case {request}: DescribeWatchlist must return the stored watchlist, got {}",
                described.raw
            );
            expected_ids.push(watchlist_id);
        }

        let listed = emulator
            .call("ListWatchlists", &json!({"DomainId": domain_id}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {extra:?}: ListWatchlists must succeed, got {} {}",
            listed.status, listed.raw
        );
        let summaries = listed.body["WatchlistSummaries"]
            .as_array()
            .expect("WatchlistSummaries is a list")
            .clone();
        let listed_ids: Vec<String> = summaries
            .iter()
            .map(|summary| string_at(summary, "WatchlistId").to_string())
            .collect();
        assert_eq!(
            listed_ids, expected_ids,
            "case {extra:?}: ListWatchlists must report the default watchlist and every created one, got {}",
            listed.raw
        );
        let defaults: Vec<&Value> = summaries
            .iter()
            .filter(|summary| summary["DefaultWatchlist"] == json!(true))
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "case {extra:?}: exactly one watchlist of a domain is the default one, got {}",
            listed.raw
        );
        assert_eq!(
            defaults[0]["WatchlistId"],
            json!(default_watchlist_id),
            "case {extra:?}: the default watchlist must be the one WatchlistDetails names, got {}",
            listed.raw
        );
    }
}

/// Page through a list operation with `page_size`, and answer the ids in the
/// order they were served. Asserts the token is present only while more remain.
async fn page_through(
    emulator: &Emulator,
    target: &str,
    base: &Value,
    items_member: &str,
    id_member: &str,
    page_size: Option<i64>,
    total: usize,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    // One page per item is the worst case; anything more is a token that never
    // finishes.
    for _ in 0..=total {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let reply = emulator.call(target, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {target} page_size={page_size:?}: must succeed, got {} {}",
            reply.status, reply.raw
        );
        let page = reply.body[items_member]
            .as_array()
            .unwrap_or_else(|| panic!("case {target}: {items_member} must be a list, got {}", reply.raw))
            .clone();
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {target} page_size={size}: a page must not exceed MaxResults, got {} entries",
                page.len()
            );
        }
        ids.extend(
            page.iter()
                .map(|item| string_at(item, id_member).to_string()),
        );
        next_token = reply.body["NextToken"].as_str().map(str::to_string);
        let remaining = total.saturating_sub(ids.len());
        assert_eq!(
            next_token.is_some(),
            remaining > 0,
            "case {target} page_size={page_size:?}: NextToken must be present exactly while entries remain \
             ({remaining} left), got {}",
            reply.raw
        );
        if next_token.is_none() {
            break;
        }
    }
    assert!(
        next_token.is_none(),
        "case {target} page_size={page_size:?}: paging did not terminate"
    );
    ids
}

#[tokio::test]
async fn list_domains_pages_the_stored_domains_in_creation_order() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let domain = emulator
            .create_domain(&json!({
                "Name": format!("paged-domain-{index}"),
                "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
            }))
            .await;
        created.push(string_at(&domain, "DomainId").to_string());
    }

    // MaxResults' modelled range is 1..10: both published bounds, values inside
    // it, and the request that omits it.
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(10), None] {
        let paged = page_through(
            &emulator,
            "ListDomains",
            &json!({}),
            "DomainSummaries",
            "DomainId",
            page_size,
            created.len(),
        )
        .await;
        assert_eq!(
            paged, created,
            "case page_size={page_size:?}: paging must yield every stored domain once, in creation order"
        );
    }

    // One past the published maximum is refused by the modelled constraint.
    let refused = emulator
        .call("ListDomains", &json!({"MaxResults": 11}))
        .await;
    assert_eq!(
        refused.status, 400,
        "case MaxResults=11: a value past the modelled range is refused, got {} {}",
        refused.status, refused.raw
    );
    assert!(
        refused.error_code().contains("ValidationException"),
        "case MaxResults=11: the refusal is a ValidationException, got {}",
        refused.raw
    );
}

#[tokio::test]
async fn list_watchlists_pages_a_domains_watchlists_in_creation_order() {
    let emulator = Emulator::new();
    let domain = emulator
        .create_domain(&json!({
            "Name": "paged-watchlists",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }))
        .await;
    let domain_id = string_at(&domain, "DomainId").to_string();
    let mut created = vec![string_at(&domain["WatchlistDetails"], "DefaultWatchlistId").to_string()];
    for index in 0..5 {
        let reply = emulator
            .call(
                "CreateWatchlist",
                &json!({"DomainId": domain_id, "Name": format!("list-{index}")}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case watchlist {index}: CreateWatchlist must succeed, got {} {}",
            reply.status, reply.raw
        );
        created.push(string_at(&reply.body["Watchlist"], "WatchlistId").to_string());
    }

    // MaxResults' modelled range here is 1..100.
    for page_size in [Some(1), Some(2), Some(4), Some(6), Some(100), None] {
        let paged = page_through(
            &emulator,
            "ListWatchlists",
            &json!({"DomainId": domain_id}),
            "WatchlistSummaries",
            "WatchlistId",
            page_size,
            created.len(),
        )
        .await;
        assert_eq!(
            paged, created,
            "case page_size={page_size:?}: paging must yield every stored watchlist once, in creation order"
        );
    }

    let refused = emulator
        .call(
            "ListWatchlists",
            &json!({"DomainId": domain_id, "MaxResults": 101}),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case MaxResults=101: a value past the modelled range is refused, got {} {}",
        refused.status, refused.raw
    );
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_resource_it_created() {
    let emulator = Emulator::new();

    // Rows differ in the token's shape; each is replayed and each must answer
    // with the resource its first call created.
    for token in ["t", "replay-token_1", &"z".repeat(64)] {
        let request = json!({
            "Name": "idempotent-domain",
            "ClientToken": token,
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        });
        let first = emulator.create_domain(&request).await;
        let replayed = emulator.create_domain(&request).await;
        assert_eq!(
            replayed, first,
            "case token={token}: a replayed CreateDomain must return the first domain"
        );

        let listed = emulator.call("ListDomains", &json!({})).await;
        let occurrences = listed.body["DomainSummaries"]
            .as_array()
            .expect("DomainSummaries is a list")
            .iter()
            .filter(|summary| summary["DomainId"] == first["DomainId"])
            .count();
        assert_eq!(
            occurrences, 1,
            "case token={token}: the replay must not have stored a second domain, got {}",
            listed.raw
        );

        let domain_id = string_at(&first, "DomainId").to_string();
        let watchlist_request =
            json!({"DomainId": domain_id, "Name": "idempotent-list", "ClientToken": token});
        let first_watchlist = emulator.call("CreateWatchlist", &watchlist_request).await;
        let replayed_watchlist = emulator.call("CreateWatchlist", &watchlist_request).await;
        assert_eq!(
            replayed_watchlist.body["Watchlist"], first_watchlist.body["Watchlist"],
            "case token={token}: a replayed CreateWatchlist must return the first watchlist"
        );

        // A different token in the same domain mints a second watchlist.
        let other = emulator
            .call(
                "CreateWatchlist",
                &json!({
                    "DomainId": domain_id,
                    "Name": "idempotent-list",
                    "ClientToken": format!("{token}-other")
                }),
            )
            .await;
        assert_ne!(
            other.body["Watchlist"]["WatchlistId"], first_watchlist.body["Watchlist"]["WatchlistId"],
            "case token={token}: a different ClientToken must mint a new watchlist"
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_resource_is_a_404_and_a_malformed_one_is_a_400() {
    let emulator = Emulator::new();
    let domain = emulator
        .create_domain(&json!({
            "Name": "error-domain",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }))
        .await;
    let domain_id = string_at(&domain, "DomainId").to_string();

    // Well-formed ids that name nothing: the modelled 404.
    let not_found: Vec<(&str, Value)> = vec![
        ("DescribeDomain", json!({"DomainId": ABSENT_DOMAIN_ID})),
        ("ListWatchlists", json!({"DomainId": ABSENT_DOMAIN_ID})),
        (
            "CreateWatchlist",
            json!({"DomainId": ABSENT_DOMAIN_ID, "Name": "orphan"}),
        ),
        ("DeleteDomain", json!({"DomainId": ABSENT_DOMAIN_ID})),
        (
            "DescribeWatchlist",
            json!({"DomainId": ABSENT_DOMAIN_ID, "WatchlistId": ABSENT_DOMAIN_ID}),
        ),
        // The domain exists; the watchlist does not.
        (
            "DescribeWatchlist",
            json!({"DomainId": domain_id, "WatchlistId": ABSENT_DOMAIN_ID}),
        ),
    ];
    for (target, request) in not_found {
        let reply = emulator.call(target, &request).await;
        assert_eq!(
            reply.status, 404,
            "case {target} {request}: an id naming no resource is a 404, got {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.error_code().contains("ResourceNotFoundException"),
            "case {target} {request}: the error is a ResourceNotFoundException, got {}",
            reply.raw
        );
    }

    // Ids outside the modelled 22-character pattern: refused as invalid, never
    // as "not found" and never as 501.
    let malformed = ["too-short", "", &"a".repeat(23), "has-a-dash-in-22-chars"];
    for id in malformed {
        let reply = emulator
            .call("DescribeDomain", &json!({"DomainId": id}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case DomainId={id:?}: an id outside ^[a-zA-Z0-9]{{22}}$ is refused with 400, got {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.error_code().contains("ValidationException"),
            "case DomainId={id:?}: the refusal is a ValidationException, got {}",
            reply.raw
        );
    }

    // A pagination token that names nothing is refused, not answered with a
    // silently empty page.
    let bad_token = emulator
        .call("ListDomains", &json!({"NextToken": "not-a-token"}))
        .await;
    assert_eq!(
        bad_token.status, 400,
        "case NextToken=not-a-token: an unresumable token is refused with 400, got {} {}",
        bad_token.status, bad_token.raw
    );
}

#[tokio::test]
async fn the_store_is_bounded_and_a_create_past_the_bound_is_refused() {
    use service_com_amazonaws_voiceid_voiceid::state::{
        ComAmazonawsVoiceidVoiceidState, IdempotencyKey, MAX_DOMAINS,
        MAX_REMEMBERED_CLIENT_TOKENS, MAX_WATCHLISTS_PER_DOMAIN,
    };

    let emulator = Emulator::new();

    // Filling the domain store to its bound succeeds; the next create is
    // refused with the modelled 402 rather than growing memory without limit.
    let mut last_domain_id = String::new();
    for index in 0..MAX_DOMAINS {
        let reply = emulator
            .call(
                "CreateDomain",
                &json!({
                    "Name": format!("bounded-{index}"),
                    "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case domain {index} of {MAX_DOMAINS}: must succeed, got {} {}",
            reply.status, reply.raw
        );
        last_domain_id = string_at(&reply.body["Domain"], "DomainId").to_string();
    }
    let refused = emulator
        .call(
            "CreateDomain",
            &json!({
                "Name": "one-too-many",
                "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
            }),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "case domain {}: past the bound the create is a ServiceQuotaExceededException, got {} {}",
        MAX_DOMAINS + 1,
        refused.status,
        refused.raw
    );
    assert!(
        refused.error_code().contains("ServiceQuotaExceededException"),
        "case domain {}: the refusal names the quota error, got {}",
        MAX_DOMAINS + 1,
        refused.raw
    );

    // Deleting one makes room again: the bound is a capacity guard, not a
    // one-way latch.
    let deleted = emulator
        .call("DeleteDomain", &json!({"DomainId": last_domain_id}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain: must succeed, got {} {}",
        deleted.status, deleted.raw
    );
    let after_delete = emulator
        .call(
            "CreateDomain",
            &json!({
                "Name": "room-again",
                "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
            }),
        )
        .await;
    assert_eq!(
        after_delete.status, 200,
        "case create after delete: a freed slot must be usable, got {} {}",
        after_delete.status, after_delete.raw
    );

    // The same holds per domain for watchlists: the default watchlist counts,
    // so MAX_WATCHLISTS_PER_DOMAIN - 1 more fit.
    let domain_id = string_at(&after_delete.body["Domain"], "DomainId").to_string();
    for index in 0..(MAX_WATCHLISTS_PER_DOMAIN - 1) {
        let reply = emulator
            .call(
                "CreateWatchlist",
                &json!({"DomainId": domain_id, "Name": format!("bounded-{index}")}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case watchlist {index} of {MAX_WATCHLISTS_PER_DOMAIN}: must succeed, got {} {}",
            reply.status, reply.raw
        );
    }
    let refused_watchlist = emulator
        .call(
            "CreateWatchlist",
            &json!({"DomainId": domain_id, "Name": "one-too-many"}),
        )
        .await;
    assert_eq!(
        refused_watchlist.status, 402,
        "case watchlist {}: past the bound the create is a ServiceQuotaExceededException, got {} {}",
        MAX_WATCHLISTS_PER_DOMAIN + 1,
        refused_watchlist.status,
        refused_watchlist.raw
    );

    // The remembered-client-token map is bounded too, evicting oldest-first.
    // Driven on the state directly: reaching MAX_REMEMBERED_CLIENT_TOKENS over
    // HTTP would need that many creates, which the store bound above forbids.
    let mut state = ComAmazonawsVoiceidVoiceidState::default();
    let overflow = 10;
    for index in 0..(MAX_REMEMBERED_CLIENT_TOKENS + overflow) {
        state.remember_token(
            IdempotencyKey::domain(&format!("token-{index}")),
            format!("resource-{index}"),
        );
    }
    assert_eq!(
        state.idempotency.len(),
        MAX_REMEMBERED_CLIENT_TOKENS,
        "case {} tokens: the map must stay at its bound, got {}",
        MAX_REMEMBERED_CLIENT_TOKENS + overflow,
        state.idempotency.len()
    );
    for index in 0..overflow {
        assert!(
            !state
                .idempotency
                .contains_key(&IdempotencyKey::domain(&format!("token-{index}"))),
            "case token-{index}: the oldest tokens must be the evicted ones"
        );
    }
    for index in overflow..(MAX_REMEMBERED_CLIENT_TOKENS + overflow) {
        assert!(
            state
                .idempotency
                .contains_key(&IdempotencyKey::domain(&format!("token-{index}"))),
            "case token-{index}: a token within the bound must still be remembered"
        );
    }
    // Re-remembering a known token is an update, not a second entry.
    state.remember_token(
        IdempotencyKey::domain(&format!("token-{}", MAX_REMEMBERED_CLIENT_TOKENS)),
        "resource-updated".to_string(),
    );
    assert_eq!(
        state.idempotency.len(),
        MAX_REMEMBERED_CLIENT_TOKENS,
        "case re-remembered token: the map must not grow, got {}",
        state.idempotency.len()
    );
}

#[tokio::test]
async fn deleting_a_domain_removes_it_and_its_watchlists() {
    let emulator = Emulator::new();
    let kept = emulator
        .create_domain(&json!({
            "Name": "kept-domain",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }))
        .await;
    let doomed = emulator
        .create_domain(&json!({
            "Name": "doomed-domain",
            "ServerSideEncryptionConfiguration": {"KmsKeyId": "alias/aws/voiceid"}
        }))
        .await;
    let doomed_id = string_at(&doomed, "DomainId").to_string();
    let doomed_watchlist = emulator
        .call(
            "CreateWatchlist",
            &json!({"DomainId": doomed_id, "Name": "doomed-list"}),
        )
        .await;
    let doomed_watchlist_id =
        string_at(&doomed_watchlist.body["Watchlist"], "WatchlistId").to_string();

    let deleted = emulator
        .call("DeleteDomain", &json!({"DomainId": doomed_id}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain: must succeed, got {} {}",
        deleted.status, deleted.raw
    );
    assert_eq!(
        deleted.raw, "{}",
        "case DeleteDomain: the model gives this operation no output members, got {}",
        deleted.raw
    );

    // Every later call for that id answers 404 ...
    let gone: Vec<(&str, Value)> = vec![
        ("DescribeDomain", json!({"DomainId": doomed_id})),
        ("ListWatchlists", json!({"DomainId": doomed_id})),
        (
            "DescribeWatchlist",
            json!({"DomainId": doomed_id, "WatchlistId": doomed_watchlist_id}),
        ),
        ("DeleteDomain", json!({"DomainId": doomed_id})),
    ];
    for (target, request) in gone {
        let reply = emulator.call(target, &request).await;
        assert_eq!(
            reply.status, 404,
            "case {target} after delete: must be a 404, got {} {}",
            reply.status, reply.raw
        );
    }

    // ... and the domain is gone from ListDomains, while its neighbour stays.
    let listed = emulator.call("ListDomains", &json!({})).await;
    let listed_ids: Vec<String> = listed.body["DomainSummaries"]
        .as_array()
        .expect("DomainSummaries is a list")
        .iter()
        .map(|summary| string_at(summary, "DomainId").to_string())
        .collect();
    assert_eq!(
        listed_ids,
        vec![string_at(&kept, "DomainId").to_string()],
        "case ListDomains after delete: only the surviving domain is listed, got {}",
        listed.raw
    );
}
