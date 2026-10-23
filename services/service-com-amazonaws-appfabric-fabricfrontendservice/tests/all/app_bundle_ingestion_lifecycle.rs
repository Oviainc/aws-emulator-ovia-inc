// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppFabric's app-bundle / ingestion lifecycle, driven over
//! the wire (restJson1) through the same router the emulator boots.
//!
//! Each test states a RULE over a table of inputs rather than one recorded call:
//! identifier resolution accepts both modelled forms, a listing pages through
//! every record exactly once, a child belongs only to its parent, and an
//! idempotency token replays its resource instead of minting a second one.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is how the router picks
/// the claiming service. No authenticator is registered, so the signature itself
/// is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/appfabric/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region the `AUTHORIZATION` scope above signs for, which is therefore the
/// region every ARN the emulator mints under it carries.
const REGION: &str = "us-east-1";

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// An `appfabric` ARN for a resource no create call ever minted, in the form
/// `CreateIngestion` builds (`appbundle/<uuid>/ingestion/<uuid>`).
pub(crate) fn unknown_ingestion_arn(bundle_uuid: &str, ingestion_uuid: &str) -> String {
    format!(
        "arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{bundle_uuid}/ingestion/{ingestion_uuid}"
    )
}

/// An `appfabric` app-bundle ARN for a bundle no create call ever minted.
pub(crate) fn unknown_bundle_arn(bundle_uuid: &str) -> String {
    format!("arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{bundle_uuid}")
}

/// One emulator: the shared engine plus this service mounted on a router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

/// A response, reduced to what these tests assert on.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`) of an error envelope, or `""`.
    pub(crate) fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        Self::with_engine(&Arc::new(engine::InMemoryEngine::new()))
    }

    /// The same emulator over an engine the CALLER keeps a handle to, so a test
    /// can also reach the store the handlers write through
    /// (`state::service_state`) — the only way to set up a resource state no
    /// implemented operation can reach yet.
    pub(crate) fn with_engine(engine: &Arc<engine::InMemoryEngine>) -> Self {
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(engine)));
        Self { router }
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }

    pub(crate) async fn create_app_bundle(&self, body: Value) -> Answer {
        self.call("POST", "/appbundles", Some(body)).await
    }

    async fn get_app_bundle(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/appbundles/{}", encode(identifier)), None)
            .await
    }

    async fn delete_app_bundle(&self, identifier: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/appbundles/{}", encode(identifier)),
            None,
        )
        .await
    }

    async fn list_app_bundles(&self, query: &str) -> Answer {
        self.call("GET", &format!("/appbundles{query}"), None).await
    }

    /// The dependency AppFabric's resource chain puts BEFORE an ingestion: the
    /// bundle must already hold an app authorization for the ingestion's
    /// (`app`, `tenantId`) pair that has completed the connect handshake
    /// (authorize → connect → ingest).
    ///
    /// Every failure here is ignored on purpose: a caller may be exercising a
    /// bundle that does not exist, or a pair this bundle already holds an
    /// authorization for. What a test asserts on is always the call that
    /// follows.
    pub(crate) async fn authorize(&self, bundle: &str, app: &str, tenant: &str) {
        let created = self
            .call(
                "POST",
                &format!("/appbundles/{}/appauthorizations", encode(bundle)),
                Some(json!({
                    "app": app,
                    "authType": "apiKey",
                    "credential": {"apiKeyCredential": {"apiKey": "placeholder-api-key"}},
                    "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": tenant},
                })),
            )
            .await;
        let Some(authorization) = created
            .body
            .get("appAuthorization")
            .and_then(|authorization| authorization.get("appAuthorizationArn"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let _ = self
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/appauthorizations/{}/connect",
                    encode(bundle),
                    encode(authorization)
                ),
                Some(json!({})),
            )
            .await;
    }

    pub(crate) async fn create_ingestion(&self, bundle: &str, body: Value) -> Answer {
        // Satisfy the ingestion's app-authorization dependency first, so a test
        // about ingestions is not a test about the chain that precedes them.
        if let (Some(app), Some(tenant)) = (
            body.get("app").and_then(Value::as_str),
            body.get("tenantId").and_then(Value::as_str),
        ) {
            self.authorize(bundle, app, tenant).await;
        }
        self.call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_ingestion(&self, bundle: &str, ingestion: &str) -> Answer {
        self.call(
            "GET",
            &format!(
                "/appbundles/{}/ingestions/{}",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
    }

    pub(crate) async fn list_ingestions(&self, bundle: &str, query: &str) -> Answer {
        self.call(
            "GET",
            &format!("/appbundles/{}/ingestions{query}", encode(bundle)),
            None,
        )
        .await
    }

    /// `DELETE /appbundles/{appBundleIdentifier}/ingestions/{ingestionIdentifier}`.
    pub(crate) async fn delete_ingestion(&self, bundle: &str, ingestion: &str) -> Answer {
        self.call(
            "DELETE",
            &format!(
                "/appbundles/{}/ingestions/{}",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
    }
}

/// Percent-encode an `@httpLabel` or query value the way an AWS SDK does: every
/// byte outside RFC 3986's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" /
/// "~"`) becomes `%XX`. An identifier in its ARN form carries `:` and `/`, and
/// `/` would otherwise split a non-greedy label across two path segments.
pub(crate) fn encode(label: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(label.len());
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            // Infallible: writing to a String never fails.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// A fresh client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built
/// from `seed` so a test can name the same token twice.
fn client_token(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// A UUID-shaped identifier no create call ever minted.
pub(crate) fn unknown_uuid() -> String {
    "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string()
}

pub(crate) fn arn_of(body: &Value, resource: &str) -> String {
    body.get(resource)
        .and_then(|r| r.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected {resource}.arn in {body}"))
        .to_string()
}

/// The bare UUID at the end of an ARN, which is the other identifier form the
/// model's `Identifier` shape accepts for the same resource.
pub(crate) fn uuid_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("an ARN with a resource id, got {arn}"))
        .to_string()
}

/// RULE: a created app bundle is PERSISTED, and `GetAppBundle` reports the stored
/// record for either identifier form (ARN or bare UUID). The table spans the
/// optional members: no customer-managed key, a key named by bare id, a key named
/// by ARN, no tags, and several tags.
#[tokio::test]
async fn an_app_bundle_round_trips_by_arn_and_by_uuid() {
    let emulator = Emulator::new();

    // (case, request body, expected customerManagedKeyArn)
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        ("no optional members", json!({}), None),
        (
            "no key, two tags",
            json!({"tags": [{"key": "env", "value": "prod"}, {"key": "team", "value": "data"}]}),
            None,
        ),
        (
            "key by bare id",
            json!({"customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-4333-8444-555555555555"
            )),
        ),
        (
            "key by ARN",
            json!({"customerManagedKeyIdentifier":
                format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/66666666-7777-4888-8999-aaaaaaaaaaaa")}),
            Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/66666666-7777-4888-8999-aaaaaaaaaaaa"
            )),
        ),
        (
            "empty tag list",
            json!({"tags": []}),
            None,
        ),
    ];

    for (case, body, expected_key_arn) in cases {
        let created = emulator.create_app_bundle(body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "appBundle");
        let id = uuid_of(&arn);
        assert_eq!(
            arn,
            format!("arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{id}"),
            "case {case:?}: expected a real appbundle ARN, got {arn:?}"
        );

        for identifier in [arn.clone(), id.clone()] {
            let read = emulator.get_app_bundle(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get("appBundle"),
                created.body.get("appBundle"),
                "case {case:?} read by {identifier:?}: expected the stored record {}, got {}",
                created.body,
                read.body
            );
            assert_eq!(
                read.body
                    .get("appBundle")
                    .and_then(|b| b.get("customerManagedKeyArn"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                expected_key_arn,
                "case {case:?} read by {identifier:?}: wrong customerManagedKeyArn in {}",
                read.body
            );
        }
    }
}

/// RULE: an unknown identifier is `ResourceNotFoundException` (HTTP 404), in
/// either identifier form, and whichever resource it names.
#[tokio::test]
async fn an_unknown_identifier_is_not_found() {
    let emulator = Emulator::new();
    let bundle_arn = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let unknown = unknown_uuid();
    let unknown_arn = format!("arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{unknown}");

    // (case, answer)
    let cases = vec![
        (
            "GetAppBundle by unknown uuid",
            emulator.get_app_bundle(&unknown).await,
        ),
        (
            "GetAppBundle by unknown arn",
            emulator.get_app_bundle(&unknown_arn).await,
        ),
        (
            "ListIngestions of an unknown bundle",
            emulator.list_ingestions(&unknown, "").await,
        ),
        (
            "CreateIngestion under an unknown bundle",
            emulator
                .create_ingestion(
                    &unknown,
                    json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
                )
                .await,
        ),
        (
            "CreateIngestion under an unknown bundle arn",
            emulator
                .create_ingestion(
                    &unknown_arn,
                    json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
                )
                .await,
        ),
        (
            "GetIngestion of an unknown ingestion in a real bundle",
            emulator.get_ingestion(&bundle_arn, &unknown).await,
        ),
    ];

    for (case, answer) in cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
    }
}

/// RULE: a listing pages through every stored record exactly once, in creation
/// order, honours `maxResults` as an upper bound on the page, and omits
/// `nextToken` on the last page. The table spans the store size and the page
/// size, including an empty store and a page size that divides the total exactly.
#[tokio::test]
async fn list_app_bundles_pages_through_every_bundle_once() {
    // (case, bundles to create, maxResults)
    let cases: Vec<(&str, usize, Option<u32>)> = vec![
        ("empty store, no page size", 0, None),
        ("empty store, page size 1", 0, Some(1)),
        ("one bundle, page size 1", 1, Some(1)),
        ("three bundles, page size 1", 3, Some(1)),
        ("four bundles, page size 2 divides exactly", 4, Some(2)),
        ("five bundles, page size 3", 5, Some(3)),
        ("seven bundles, no page size", 7, None),
        ("two bundles, page size above the total", 2, Some(100)),
    ];

    for (case, total, max_results) in cases {
        let emulator = Emulator::new();
        let mut created: Vec<String> = Vec::new();
        for _ in 0..total {
            created.push(arn_of(
                &emulator.create_app_bundle(json!({})).await.body,
                "appBundle",
            ));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(max) = max_results {
                query.push(format!("maxResults={max}"));
            }
            if let Some(ref next) = token {
                query.push(format!("nextToken={}", encode(next)));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let answer = emulator.list_app_bundles(&query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                answer.status, answer.body
            );
            let page: Vec<String> = answer
                .body
                .get("appBundleSummaryList")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no appBundleSummaryList in {}", answer.body))
                .iter()
                .map(|summary| {
                    summary
                        .get("arn")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("case {case:?}: a summary without an arn in {}", answer.body))
                        .to_string()
                })
                .collect();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max as usize,
                    "case {case:?}: page of {} exceeds maxResults {max}",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= total + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );

            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case {case:?}: expected every bundle once in creation order {created:?}, got {seen:?}"
        );
    }
}

/// RULE: a `nextToken` the service did not issue is refused as a
/// `ValidationException` (HTTP 400), never silently treated as the first page.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.create_app_bundle(json!({})).await;

    // Each row is a token shaped unlike anything `encode_page_token` emits.
    for token in [
        "not-a-token",
        "v1-",
        "v2-0000000000000001",
        "0",
        "v1-zzzz",
        // The old, unscoped rendering: a bare sequence names no listing.
        "v1-0000000000000001",
        // The right listing, a sequence that is not hex.
        "v1-appbundles-zzzzzzzzzzzzzzzz",
        // Another listing's name entirely.
        "v1-ingestions-0000000000000001",
    ] {
        let answer = emulator
            .list_app_bundles(&format!("?nextToken={}", encode(token)))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {}",
            answer.body
        );
    }
}

/// RULE: a `nextToken` names a position WITHIN the listing that issued it, so a
/// token carried to another listing — or to another parent's listing of the same
/// kind — is refused as a `ValidationException` (HTTP 400) rather than read as a
/// position there (which would silently skip records under HTTP 200).
#[tokio::test]
async fn a_token_from_another_listing_is_refused() {
    let emulator = Emulator::new();

    // Two bundles, each with two ingestions, so every listing below has more
    // records than the page size and therefore really does issue a token.
    let mut bundles: Vec<String> = Vec::new();
    for _ in 0..2 {
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        for index in 0..2 {
            emulator
                .create_ingestion(
                    &bundle,
                    json!({"app": "SLACK", "tenantId": format!("t-{index}"),
                           "ingestionType": "auditLog"}),
                )
                .await;
        }
        bundles.push(bundle);
    }

    /// The `nextToken` a listing hands out for its first page of one record.
    fn first_token(answer: &Answer, case: &str) -> String {
        answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken in {}", answer.body))
            .to_string()
    }

    let bundle_token = first_token(
        &emulator.list_app_bundles("?maxResults=1").await,
        "ListAppBundles",
    );
    let first_ingestion_token = first_token(
        &emulator.list_ingestions(&bundles[0], "?maxResults=1").await,
        "ListIngestions on the first bundle",
    );
    let second_ingestion_token = first_token(
        &emulator.list_ingestions(&bundles[1], "?maxResults=1").await,
        "ListIngestions on the second bundle",
    );

    // (case, the listing the token is replayed against, the foreign token)
    enum Listing<'a> {
        AppBundles,
        Ingestions(&'a str),
    }
    let cases: Vec<(&str, Listing<'_>, &str)> = vec![
        (
            "an ingestion token on ListAppBundles",
            Listing::AppBundles,
            first_ingestion_token.as_str(),
        ),
        (
            "a bundle token on ListIngestions",
            Listing::Ingestions(&bundles[0]),
            bundle_token.as_str(),
        ),
        (
            "another bundle's ingestion token on ListIngestions",
            Listing::Ingestions(&bundles[0]),
            second_ingestion_token.as_str(),
        ),
        (
            "the first bundle's ingestion token on the second bundle",
            Listing::Ingestions(&bundles[1]),
            first_ingestion_token.as_str(),
        ),
    ];

    for (case, listing, token) in cases {
        let query = format!("?maxResults=1&nextToken={}", encode(token));
        let answer = match listing {
            Listing::AppBundles => emulator.list_app_bundles(&query).await,
            Listing::Ingestions(bundle) => emulator.list_ingestions(bundle, &query).await,
        };
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for token {token:?}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException for token {token:?}, got {}",
            answer.body
        );
    }

    // ...and each listing still accepts its OWN token, so the check above
    // refuses only foreign ones.
    let answer = emulator
        .list_app_bundles(&format!("?maxResults=1&nextToken={}", encode(&bundle_token)))
        .await;
    assert_eq!(
        answer.status, 200,
        "a listing must accept the token it issued, got {} with body {}",
        answer.status, answer.body
    );
    let answer = emulator
        .list_ingestions(
            &bundles[0],
            &format!("?maxResults=1&nextToken={}", encode(&first_ingestion_token)),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "a listing must accept the token it issued, got {} with body {}",
        answer.status, answer.body
    );
}

/// RULE: every resource id the service mints is a version-4 UUID in the
/// lowercase hyphenated form the model's `UUID`/`Identifier` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, RFC 9562
/// §5.4: version nibble `4`, variant nibble in `8..=b`), and no two resources
/// ever share one. Checked over a batch of bundles and of ingestions, because
/// uniqueness is a property of the minting, not of one call.
#[tokio::test]
async fn every_minted_id_is_a_distinct_version_4_uuid() {
    /// The model's `UUID` pattern, as a check over the rendered form.
    fn is_v4_uuid(id: &str) -> bool {
        let groups: Vec<&str> = id.split('-').collect();
        groups.len() == 5
            && [8_usize, 4, 4, 4, 12]
                .iter()
                .zip(&groups)
                .all(|(len, group)| {
                    group.len() == *len
                        && group
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            // Version 4 is the first nibble of the third group, and the variant
            // nibble (the first of the fourth group) is one of 8, 9, a, b.
            && groups[2].starts_with('4')
            && groups[3].starts_with(['8', '9', 'a', 'b'])
    }

    let emulator = Emulator::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut minted = 0_usize;

    for _ in 0..8 {
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let bundle_id = uuid_of(&bundle);
        assert!(
            is_v4_uuid(&bundle_id),
            "case app bundle: expected a version-4 UUID id, got {bundle_id:?} (from {bundle:?})"
        );
        ids.insert(bundle_id);
        minted += 1;

        for index in 0..4 {
            let ingestion = arn_of(
                &emulator
                    .create_ingestion(
                        &bundle,
                        json!({"app": "SLACK", "tenantId": format!("t-{index}"),
                               "ingestionType": "auditLog"}),
                    )
                    .await
                    .body,
                "ingestion",
            );
            let ingestion_id = ingestion
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_v4_uuid(&ingestion_id),
                "case ingestion: expected a version-4 UUID id, got {ingestion_id:?} (from {ingestion:?})"
            );
            ids.insert(ingestion_id);
            minted += 1;
        }
    }

    assert_eq!(
        ids.len(),
        minted,
        "expected {minted} distinct ids, got {} — ids must never repeat: {ids:?}",
        ids.len()
    );
}

/// RULE: an ingestion is created under ONE app bundle, starts in the `disabled`
/// state with real timestamps, and is visible only through that bundle. The table
/// spans the applications and tenants AppFabric's `app` member documents.
#[tokio::test]
async fn an_ingestion_belongs_only_to_its_own_bundle() {
    let emulator = Emulator::new();
    let first = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let second = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );

    // `app` values from the model's CreateIngestionRequest$app doc ("Valid values
    // are: SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX
    // SMARTSHEET CISCO"), including entries the issue did not cite.
    let rows: Vec<(&str, &str)> = vec![
        ("SLACK", "t-1"),
        ("ASANA", "tenant-with-a-much-longer-identifier"),
        ("M365AUDITLOGS", "0"),
        ("CISCO", "tenant.with.dots"),
    ];

    let mut expected_first: Vec<String> = Vec::new();
    for (app, tenant) in &rows {
        let created = emulator
            .create_ingestion(
                &first,
                json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"}),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {app:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let ingestion = created
            .body
            .get("ingestion")
            .unwrap_or_else(|| panic!("case {app:?}: no ingestion in {}", created.body));
        assert_eq!(
            ingestion.get("appBundleArn").and_then(Value::as_str),
            Some(first.as_str()),
            "case {app:?}: expected appBundleArn {first:?}, got {ingestion}"
        );
        assert_eq!(
            ingestion.get("state").and_then(Value::as_str),
            Some("disabled"),
            "case {app:?}: a new ingestion must start disabled, got {ingestion}"
        );
        assert_eq!(
            ingestion.get("ingestionType").and_then(Value::as_str),
            Some("auditLog"),
            "case {app:?}: wrong ingestionType in {ingestion}"
        );
        for stamp in ["createdAt", "updatedAt"] {
            let seconds = ingestion
                .get(stamp)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {app:?}: no numeric {stamp} in {ingestion}"));
            assert!(
                seconds > 1.0e9,
                "case {app:?}: {stamp} must be a real epoch timestamp, got {seconds}"
            );
        }

        let arn = arn_of(&created.body, "ingestion");
        expected_first.push(arn.clone());

        // Round trip: the full record reads back through GetIngestion, by either
        // identifier form, and only under its own bundle.
        for identifier in [arn.clone(), uuid_of(&arn)] {
            let read = emulator.get_ingestion(&first, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {app:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get("ingestion"),
                created.body.get("ingestion"),
                "case {app:?} read by {identifier:?}: expected the stored record {}, got {}",
                created.body,
                read.body
            );

            let crossed = emulator.get_ingestion(&second, &identifier).await;
            assert_eq!(
                crossed.status, 404,
                "case {app:?}: {identifier:?} must not resolve under another bundle, got {} with body {}",
                crossed.status, crossed.body
            );
        }
    }

    let listed = emulator.list_ingestions(&first, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    let arns: Vec<String> = listed
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no ingestions in {}", listed.body))
        .iter()
        .map(|summary| {
            summary
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        arns, expected_first,
        "expected this bundle's ingestions in creation order {expected_first:?}, got {arns:?}"
    );
    let summary_apps: Vec<String> = listed
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| {
            summary
                .get("app")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        summary_apps,
        rows.iter().map(|(app, _)| (*app).to_string()).collect::<Vec<_>>(),
        "a summary must carry the app its record was created with, got {}",
        listed.body
    );

    let other = emulator.list_ingestions(&second, "").await;
    let other_arns = other
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("no ingestions in {}", other.body));
    assert_eq!(
        other_arns, 0,
        "a sibling bundle must list none of these ingestions, got {}",
        other.body
    );
}

/// RULE: `ListIngestions` pages the ingestions of its own bundle the same way
/// `ListAppBundles` pages bundles.
#[tokio::test]
async fn list_ingestions_pages_through_every_ingestion_once() {
    for (case, total, max_results) in [
        ("one ingestion, page size 1", 1_usize, 1_u32),
        ("three ingestions, page size 1", 3, 1),
        ("four ingestions, page size 2", 4, 2),
        ("five ingestions, page size 100", 5, 100),
    ] {
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            created.push(arn_of(
                &emulator
                    .create_ingestion(
                        &bundle,
                        json!({"app": "SLACK", "tenantId": format!("t-{index}"),
                               "ingestionType": "auditLog"}),
                    )
                    .await
                    .body,
                "ingestion",
            ));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = format!("?maxResults={max_results}");
            if let Some(ref next) = token {
                query.push_str(&format!("&nextToken={}", encode(next)));
            }
            let answer = emulator.list_ingestions(&bundle, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                answer.status, answer.body
            );
            let page = answer
                .body
                .get("ingestions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no ingestions in {}", answer.body));
            assert!(
                page.len() <= max_results as usize,
                "case {case:?}: page of {} exceeds maxResults {max_results}",
                page.len()
            );
            seen.extend(page.iter().map(|summary| {
                summary
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            }));
            token = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= total,
                "case {case:?}: pagination returned more than the {total} stored ingestions"
            );
        }

        assert_eq!(
            seen, created,
            "case {case:?}: expected every ingestion once in creation order {created:?}, got {seen:?}"
        );
    }
}

/// RULE: an idempotency token replayed with the SAME parameters replays the
/// resource the first call created; replayed with DIFFERENT parameters it is a
/// conflict (HTTP 409). Two different tokens are two resources.
#[tokio::test]
async fn an_idempotency_token_replays_instead_of_creating_again() {
    let emulator = Emulator::new();

    // (case, first body, replayed body, replay must match)
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        (
            "no optional members",
            json!({"clientToken": client_token(1)}),
            json!({"clientToken": client_token(1)}),
            true,
        ),
        (
            "same tags in another order",
            json!({"clientToken": client_token(2),
                   "tags": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}]}),
            json!({"clientToken": client_token(2),
                   "tags": [{"key": "b", "value": "2"}, {"key": "a", "value": "1"}]}),
            true,
        ),
        (
            "same key identifier",
            json!({"clientToken": client_token(3),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            json!({"clientToken": client_token(3),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            true,
        ),
        (
            "different key identifier",
            json!({"clientToken": client_token(4),
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            json!({"clientToken": client_token(4),
                   "customerManagedKeyIdentifier": "66666666-7777-4888-8999-aaaaaaaaaaaa"}),
            false,
        ),
        (
            "different tags",
            json!({"clientToken": client_token(5), "tags": [{"key": "a", "value": "1"}]}),
            json!({"clientToken": client_token(5), "tags": [{"key": "a", "value": "2"}]}),
            false,
        ),
    ];

    let mut arns: BTreeSet<String> = BTreeSet::new();
    for (case, first, replayed, must_match) in cases {
        let created = emulator.create_app_bundle(first).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "appBundle");
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: a distinct clientToken must mint a distinct ARN, got {arn:?} twice"
        );

        let replay = emulator.create_app_bundle(replayed).await;
        if must_match {
            assert_eq!(
                replay.status, 201,
                "case {case:?}: expected HTTP 201 on replay, got {} with body {}",
                replay.status, replay.body
            );
            assert_eq!(
                arn_of(&replay.body, "appBundle"),
                arn,
                "case {case:?}: a replayed token must replay {arn:?}, got {}",
                replay.body
            );
        } else {
            assert_eq!(
                replay.status, 409,
                "case {case:?}: expected HTTP 409 for a parameter mismatch, got {} with body {}",
                replay.status, replay.body
            );
            assert_eq!(
                replay.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                replay.body
            );
        }
    }

    // Every replay that matched added no bundle, and every mismatch added none:
    // the store holds exactly one bundle per distinct token.
    let listed = emulator.list_app_bundles("").await;
    let count = listed
        .body
        .get("appBundleSummaryList")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_else(|| panic!("no appBundleSummaryList in {}", listed.body));
    assert_eq!(
        count,
        arns.len(),
        "expected one bundle per distinct clientToken ({}), got {count} in {}",
        arns.len(),
        listed.body
    );
}

/// RULE: deleting an app bundle removes it and the ingestions stored under it, in
/// either identifier form, and a delete of a bundle that is already gone is still
/// a success — the model declares no not-found error for this operation and it is
/// @idempotent.
#[tokio::test]
async fn deleting_an_app_bundle_removes_it_and_its_ingestions() {
    // Each row deletes by a different identifier form; the rule must not depend
    // on which form the caller used.
    for case in ["by arn", "by uuid"] {
        let emulator = Emulator::new();
        let kept = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let doomed = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let ingestion = arn_of(
            &emulator
                .create_ingestion(
                    &doomed,
                    json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
                )
                .await
                .body,
            "ingestion",
        );
        let identifier = if case == "by arn" {
            doomed.clone()
        } else {
            uuid_of(&doomed)
        };

        let deleted = emulator.delete_app_bundle(&identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204, got {} with body {}",
            deleted.status, deleted.body
        );

        for form in [doomed.clone(), uuid_of(&doomed)] {
            let read = emulator.get_app_bundle(&form).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: {form:?} must be gone, got {} with body {}",
                read.status, read.body
            );
        }

        let gone = emulator.get_ingestion(&doomed, &ingestion).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deleted bundle's ingestion must be gone, got {} with body {}",
            gone.status, gone.body
        );

        let listed = emulator.list_app_bundles("").await;
        let arns: Vec<String> = listed
            .body
            .get("appBundleSummaryList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no appBundleSummaryList in {}", listed.body))
            .iter()
            .map(|summary| {
                summary
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            arns,
            vec![kept.clone()],
            "case {case:?}: expected only the surviving bundle, got {arns:?}"
        );

        let again = emulator.delete_app_bundle(&identifier).await;
        assert_eq!(
            again.status, 204,
            "case {case:?}: a repeated delete is idempotent, got {} with body {}",
            again.status, again.body
        );
    }
}

/// RULE: an ingestion's idempotency token is scoped to the whole request,
/// INCLUDING the parent bundle it names: replaying it under the same bundle with
/// the same parameters replays the ingestion, and replaying it under a different
/// bundle (or with a different member) is a conflict, never a second resource.
#[tokio::test]
async fn an_ingestion_token_is_scoped_to_its_parent_bundle() {
    let emulator = Emulator::new();
    let first = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let second = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let body = |token: &str| {
        json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog",
               "clientToken": token})
    };

    let created = emulator
        .create_ingestion(&first, body(&client_token(11)))
        .await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    let arn = arn_of(&created.body, "ingestion");

    // (case, bundle the replay names, request body, expected status)
    let cases: Vec<(&str, &str, Value, u16)> = vec![
        (
            "same bundle, same parameters",
            &first,
            body(&client_token(11)),
            201,
        ),
        (
            "different bundle, same parameters",
            &second,
            body(&client_token(11)),
            409,
        ),
        (
            "same bundle, different tenantId",
            &first,
            json!({"app": "SLACK", "tenantId": "t-2", "ingestionType": "auditLog",
                   "clientToken": client_token(11)}),
            409,
        ),
    ];

    for (case, bundle, request, expected) in cases {
        let answer = emulator.create_ingestion(bundle, request).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        if expected == 201 {
            assert_eq!(
                arn_of(&answer.body, "ingestion"),
                arn,
                "case {case:?}: a replayed token must replay {arn:?}, got {}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {}",
                answer.body
            );
        }
    }

    // Exactly one ingestion exists, under the bundle the first call named.
    for (case, bundle, expected) in [("first", &first, 1_usize), ("second", &second, 0)] {
        let listed = emulator.list_ingestions(bundle, "").await;
        let count = listed
            .body
            .get("ingestions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_else(|| panic!("case {case:?}: no ingestions in {}", listed.body));
        assert_eq!(
            count, expected,
            "case {case:?}: expected {expected} ingestion(s), got {} in {}",
            count, listed.body
        );
    }
}

/// RULE: the store is BOUNDED — once it holds the emulator's ceiling of records a
/// further create is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402), the way an account at its limit answers, rather than growing the
/// process without bound.
#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    use service_com_amazonaws_appfabric_fabricfrontendservice::state::MAX_APP_BUNDLES;

    let emulator = Emulator::new();
    for index in 0..MAX_APP_BUNDLES {
        let created = emulator.create_app_bundle(json!({})).await;
        assert_eq!(
            created.status, 201,
            "bundle {index}: expected HTTP 201 below the ceiling, got {} with body {}",
            created.status, created.body
        );
    }

    let refused = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        refused.status, 402,
        "expected HTTP 402 at the ceiling of {MAX_APP_BUNDLES}, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {}",
        refused.body
    );
}

/// RULE: an operation this slice did not implement still answers HTTP 501, so a
/// caller can tell "not written yet" from a working operation.
#[tokio::test]
async fn an_unimplemented_operation_still_answers_501() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let ingestion = arn_of(
        &emulator
            .create_ingestion(
                &bundle,
                json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
            )
            .await
            .body,
        "ingestion",
    );

    let answer = emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/start",
                encode(&bundle),
                encode(&ingestion)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 501,
        "expected HTTP 501 from the still-scaffolded StartIngestion, got {} with body {}",
        answer.status, answer.body
    );
    // Naming the operation is what distinguishes the scaffold's 501 from the
    // 501 a rest service renders for a path that matched no operation at all.
    let message = answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("StartIngestion"),
        "expected the scaffold 501 to name StartIngestion, got {}",
        answer.body
    );
}

/// A `tracing` subscriber that keeps every event it is handed, rendered field by
/// field, so a test can assert on the audit records the handlers emit. Written
/// against `tracing` itself (a dependency this crate already carries) rather than
/// pulling a collector crate in.
#[derive(Clone, Default)]
struct AuditCapture {
    rendered: Arc<std::sync::Mutex<Vec<String>>>,
}

impl AuditCapture {
    /// The audit records captured so far — every event carrying `audit = true`.
    fn audit_records(&self) -> Vec<String> {
        self.rendered
            .lock()
            .expect("the capture mutex is never poisoned")
            .iter()
            .filter(|rendered| rendered.contains("audit=true"))
            .cloned()
            .collect()
    }
}

impl tracing::Subscriber for AuditCapture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl tracing::field::Visit for Fields<'_> {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                use std::fmt::Write as _;
                let _ = write!(self.0, "{}={value} ", field.name());
            }

            fn record_debug(
                &mut self,
                field: &tracing::field::Field,
                value: &dyn std::fmt::Debug,
            ) {
                use std::fmt::Write as _;
                let _ = write!(self.0, "{}={value:?} ", field.name());
            }
        }

        let mut rendered = String::new();
        event.record(&mut Fields(&mut rendered));
        self.rendered
            .lock()
            .expect("the capture mutex is never poisoned")
            .push(rendered);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture, installed ONCE as the PROCESS-WIDE default subscriber.
///
/// It has to be global rather than thread-local (`subscriber::set_default`):
/// `tracing` caches each callsite's interest GLOBALLY, computed from whatever
/// subscriber the FIRST thread to reach that callsite happened to have, so with
/// the harness running these tests in parallel another test reaching an audit
/// callsite first — with no subscriber installed — caches it as "never" and a
/// thread-local capture then sees nothing. One global capture plus an
/// interest-cache rebuild removes that ordering dependency; the assertions below
/// are scoped to the ids this test minted, so records other tests produce into
/// the same capture do not affect them.
fn audit_capture() -> &'static AuditCapture {
    static CAPTURE: std::sync::OnceLock<AuditCapture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = AuditCapture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("these tests install no other global tracing subscriber");
        // Recompute interest for callsites a parallel test may already have
        // reached — and cached as "never" — before this subscriber existed.
        tracing::callsite::rebuild_interest_cache();
        capture
    })
}

/// RULE: every state-changing operation writes ONE structured audit record
/// naming the operation, the presented access key id, the server-minted resource
/// and the outcome — including the ingestion-destination create and delete; a
/// reporting operation writes none, and no record carries caller-supplied content
/// (a tag value, a customer-managed key).
#[tokio::test]
async fn state_changing_operations_write_an_audit_record() {
    let captured = audit_capture();

    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator
            .create_app_bundle(json!({"tags": [{"key": "env", "value": "prod-secret"}],
                                      "customerManagedKeyIdentifier":
                                          "11111111-2222-4333-8444-555555555555"}))
            .await
            .body,
        "appBundle",
    );
    let bundle_id = uuid_of(&bundle);
    let ingestion = arn_of(
        &emulator
            .create_ingestion(
                &bundle,
                json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"}),
            )
            .await
            .body,
        "ingestion",
    );
    let ingestion_id = ingestion.rsplit('/').next().unwrap_or_default().to_string();

    // One ingestion destination under that ingestion, so this test also covers
    // the ingestion-destination create and delete. It has to be deleted before
    // the ingestion is, which is the order AppFabric documents.
    let destinations_uri = format!(
        "/appbundles/{}/ingestions/{}/ingestiondestinations",
        encode(&bundle),
        encode(&ingestion)
    );
    let destination = arn_of(
        &emulator
            .call(
                "POST",
                &destinations_uri,
                Some(json!({
                    "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                    "destinationConfiguration": {"auditLog": {"destination":
                        {"s3Bucket": {"bucketName": "audit-logs-bucket"}}}},
                })),
            )
            .await
            .body,
        "ingestionDestination",
    );
    let destination_id = uuid_of(&destination);
    let destination_uri = format!("{destinations_uri}/{}", encode(&destination));

    // Reporting operations, which must add no audit record of their own.
    emulator.get_app_bundle(&bundle).await;
    emulator.list_app_bundles("").await;
    emulator.list_ingestions(&bundle, "").await;

    emulator.call("DELETE", &destination_uri, None).await;
    // Deleting what is already gone is a success, and is recorded as such.
    emulator.call("DELETE", &destination_uri, None).await;

    emulator.delete_ingestion(&bundle, &ingestion).await;
    // Deleting what is already gone is a success, and is recorded as such.
    emulator.delete_ingestion(&bundle, &ingestion).await;

    emulator.delete_app_bundle(&bundle).await;
    // Deleting what is already gone is a success, and is recorded as such.
    emulator.delete_app_bundle(&bundle).await;

    let records: Vec<String> = captured
        .audit_records()
        .into_iter()
        // Scoped to the resources THIS test minted, so records other tests write
        // into the same process-wide capture are none of its business.
        .filter(|rendered| {
            rendered.contains(&bundle_id)
                || rendered.contains(&ingestion_id)
                || rendered.contains(&destination_id)
        })
        .collect();

    // (case, the operation, the resource type, the resource id, the outcome)
    let cases: Vec<(&str, &str, &str, String, &str)> = vec![
        (
            "a created app bundle",
            "CreateAppBundle",
            "AppBundle",
            bundle_id.clone(),
            "created",
        ),
        (
            "a created ingestion",
            "CreateIngestion",
            "Ingestion",
            ingestion_id.clone(),
            "created",
        ),
        (
            "a created ingestion destination",
            "CreateIngestionDestination",
            "IngestionDestination",
            destination_id.clone(),
            "created",
        ),
        (
            "a deleted ingestion destination",
            "DeleteIngestionDestination",
            "IngestionDestination",
            destination_id.clone(),
            "deleted",
        ),
        (
            "a delete of an absent ingestion destination",
            "DeleteIngestionDestination",
            "IngestionDestination",
            destination.clone(),
            "already absent",
        ),
        (
            "a deleted ingestion",
            "DeleteIngestion",
            "Ingestion",
            ingestion_id.clone(),
            "deleted",
        ),
        (
            "a delete of an absent ingestion",
            "DeleteIngestion",
            "Ingestion",
            ingestion.clone(),
            "already absent",
        ),
        (
            "a deleted app bundle",
            "DeleteAppBundle",
            "AppBundle",
            bundle.clone(),
            "deleted",
        ),
        (
            "a delete of an absent app bundle",
            "DeleteAppBundle",
            "AppBundle",
            bundle.clone(),
            "already absent",
        ),
    ];

    for (case, operation, resource_type, resource_id, outcome) in &cases {
        let matching: Vec<&String> = records
            .iter()
            .filter(|rendered| {
                rendered.contains(&format!("operation={operation} "))
                    && rendered.contains(&format!("outcome={outcome} "))
                    && rendered.contains(&format!("resource_type={resource_type} "))
                    && rendered.contains(resource_id.as_str())
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one audit record for {operation}/{outcome}, got {matching:?} out of {records:?}"
        );
        assert!(
            matching[0].contains("caller_akid=AKIDEXAMPLE"),
            "case {case:?}: expected the presented access key id in the record, got {:?}",
            matching[0]
        );
    }

    assert_eq!(
        records.len(),
        cases.len(),
        "expected one audit record per state-changing call and none for a read, got {records:?}"
    );
    for record in &records {
        assert!(
            !record.contains("prod-secret") && !record.contains("11111111-2222-4333-8444"),
            "an audit record must carry no caller-supplied content, got {record:?}"
        );
    }
}
