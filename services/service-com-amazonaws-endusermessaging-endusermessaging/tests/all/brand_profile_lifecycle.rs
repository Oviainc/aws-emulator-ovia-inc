// SPDX-License-Identifier: CC0-1.0

//! The brand-profile lifecycle, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises
//! the same restJson1 path a real SDK client takes: routing, deserialization,
//! boundary constraint validation, the handler, and error rendering.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a created profile is persisted, and `GetBrandProfile` / `ListBrandProfiles`
//!   report exactly what was stored — for every name the modelled `BrandProfileName`
//!   accepts, with or without `deletionProtectionEnabled` and `tags`;
//! * a `BrandProfileIdOrArn` resolves a profile by its bare id or by its full ARN;
//! * the ARN is `arn:aws:end-user-messaging:<region>:<account>:brand-profile/<id>`
//!   for whatever region the request is signed for;
//! * `ListBrandProfiles` returns every stored profile exactly once across pages, for
//!   every page size, and emits a resumable token only while items remain;
//! * `UpdateBrandProfile` applies the members present and leaves the rest alone;
//! * `DeleteBrandProfile` removes the record, and refuses with `ConflictException`
//!   while deletion protection is on;
//! * an id that names nothing is `ResourceNotFoundException` (404), and a value
//!   outside a modelled constraint is `ValidationException` (400);
//! * a repeated `clientToken` reports the first profile instead of creating another.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_endusermessaging_endusermessaging::EmulatedService;

/// This service's sigv4 signing name, from the model (`OPERATIONS.md`, "Wire protocol").
const SIGNING_NAME: &str = "end-user-messaging";

/// The account id every generated emulator presents, as `engine` defines it.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// One response, reduced to what the assertions read.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), with any
    /// `namespace#` prefix stripped. `None` on success.
    error: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn string(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string in {}", self.body))
            .to_string()
    }

    fn number(&self, name: &str) -> f64 {
        self.member(name)
            .as_f64()
            .unwrap_or_else(|| panic!("member {name} is not a number in {}", self.body))
    }
}

/// One emulator over one shared engine: every call sees the same store.
struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    fn new() -> Self {
        Self::in_region("us-east-1")
    }

    async fn call(&self, method: &str, path_and_query: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host, Signature=00",
            self.region
        );
        let payload = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://{SIGNING_NAME}.{}.amazonaws.com{path_and_query}",
                self.region
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error = if status < 400 {
            None
        } else {
            header_error
                .or_else(|| body["__type"].as_str().map(str::to_string))
                .map(|code| {
                    code.rsplit('#')
                        .next()
                        .unwrap_or(code.as_str())
                        .split('/')
                        .next()
                        .unwrap_or(code.as_str())
                        .to_string()
                })
        };
        Reply {
            status,
            error,
            body,
        }
    }

    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v1/brand-profiles", Some(body)).await
    }

    async fn create_named(&self, name: &str) -> Reply {
        self.create(json!({ "brandProfileName": name })).await
    }

    async fn get(&self, id_or_arn: &str) -> Reply {
        self.call("GET", &format!("/v1/brand-profiles/{id_or_arn}"), None)
            .await
    }

    async fn update(&self, id_or_arn: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v1/brand-profiles/{id_or_arn}"),
            Some(body),
        )
        .await
    }

    async fn delete(&self, id_or_arn: &str) -> Reply {
        self.call("DELETE", &format!("/v1/brand-profiles/{id_or_arn}"), None)
            .await
    }

    async fn list(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v1/brand-profiles{query}"), None)
            .await
    }
}

/// The names in this table are the classes `BrandProfileName`'s modelled
/// `@length(1..64)` and `^[A-Za-z0-9_ -]*[A-Za-z0-9_-][A-Za-z0-9_ -]*$` pattern
/// accept: a single character, the alphanumeric case, every punctuation the pattern
/// names, an interior space, and exactly the maximum length.
fn accepted_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "demo-brand".to_string(),
        "My_Brand profile-1".to_string(),
        "0".to_string(),
        "A".repeat(64),
    ]
}

#[tokio::test]
async fn a_created_profile_is_persisted_and_read_back_whole() {
    let em = Emulator::new();

    for name in accepted_names() {
        for protection in [None, Some(false), Some(true)] {
            let mut body = json!({ "brandProfileName": name });
            if let Some(enabled) = protection {
                body["deletionProtectionEnabled"] = json!(enabled);
            }
            let case = format!("name={name:?} deletionProtectionEnabled={protection:?}");

            let created = em.create(body).await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected 200, got {} / {:?}",
                created.status, created.body
            );
            let id = created.string("brandProfileId");
            assert_eq!(
                created.string("brandProfileName"),
                name,
                "case {case}: the created name must be the one requested, got {:?}",
                created.body
            );
            assert_eq!(
                created.string("status"),
                "ACTIVE",
                "case {case}: a created profile is ACTIVE, got {:?}",
                created.body
            );
            assert_eq!(
                created.number("attributesCreated"),
                0.0,
                "case {case}: a fresh profile has no default attributes, got {:?}",
                created.body
            );
            assert_eq!(
                created.member("deletionProtectionEnabled"),
                &json!(protection.unwrap_or(false)),
                "case {case}: an omitted deletionProtectionEnabled defaults to false, got {:?}",
                created.body
            );
            assert_eq!(
                created.string("brandProfileArn"),
                format!("arn:aws:{SIGNING_NAME}:us-east-1:{ACCOUNT_ID}:brand-profile/{id}"),
                "case {case}: unexpected ARN, got {:?}",
                created.body
            );
            assert_eq!(
                created.number("createdAt"),
                created.number("updatedAt"),
                "case {case}: a never-updated profile has createdAt == updatedAt, got {:?}",
                created.body
            );

            // Round trip: the reader reports exactly what create stored.
            let fetched = em.get(&id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: GetBrandProfile must find the profile just created, got {} / {:?}",
                fetched.status, fetched.body
            );
            for member in [
                "brandProfileId",
                "brandProfileArn",
                "brandProfileName",
                "status",
                "deletionProtectionEnabled",
                "createdAt",
                "updatedAt",
            ] {
                assert_eq!(
                    fetched.member(member),
                    created.member(member),
                    "case {case}: GetBrandProfile disagrees with CreateBrandProfile on {member}: \
                     {:?} vs {:?}",
                    fetched.member(member),
                    created.member(member)
                );
            }

            // And the same record is what the list reports.
            let listed = em.list("").await;
            let found = listed
                .member("brandProfiles")
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: brandProfiles is not a list"))
                .iter()
                .find(|item| item["brandProfileId"] == json!(id))
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: the created profile must be listed, got {:?}",
                        listed.body
                    )
                });
            assert_eq!(
                found["brandProfileName"],
                json!(name),
                "case {case}: the listed name must match, got {found:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_profile_resolves_by_bare_id_and_by_full_arn() {
    let em = Emulator::new();
    let created = em.create_named("resolve-me").await;
    let id = created.string("brandProfileId");
    let arn = created.string("brandProfileArn");

    for reference in [id.clone(), arn.clone()] {
        let fetched = em.get(&reference).await;
        assert_eq!(
            fetched.status, 200,
            "case {reference:?}: a BrandProfileIdOrArn resolves by id or ARN, got {} / {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.string("brandProfileId"),
            id,
            "case {reference:?}: resolved the wrong profile, got {:?}",
            fetched.body
        );
    }

    // An ARN naming a profile that does not exist is still a 404, not a synthesized one.
    let bogus = format!("arn:aws:{SIGNING_NAME}:us-east-1:{ACCOUNT_ID}:brand-profile/bp-00000000000000000");
    let missing = em.get(&bogus).await;
    assert_eq!(
        missing.status, 404,
        "case {bogus:?}: an unknown ARN is ResourceNotFoundException, got {} / {:?}",
        missing.status, missing.body
    );

    // The ARN of a profile deleted afterwards no longer resolves.
    assert_eq!(
        em.delete(&arn).await.status,
        200,
        "deleting by ARN must succeed"
    );
    assert_eq!(
        em.get(&arn).await.status,
        404,
        "a deleted profile's ARN must stop resolving"
    );
}

#[tokio::test]
async fn the_arn_carries_the_region_the_request_was_signed_for() {
    // Every row is a real AWS region name; the rule is the ARN's region segment is
    // the request's, not a fixed one.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        let em = Emulator::in_region(region);
        let created = em.create_named("regional").await;
        let id = created.string("brandProfileId");
        assert_eq!(
            created.string("brandProfileArn"),
            format!("arn:aws:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:brand-profile/{id}"),
            "case {region}: the ARN must carry the signed region, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn every_page_size_walks_the_whole_list_exactly_once() {
    const PROFILES: usize = 7;
    let em = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..PROFILES {
        created.push(
            em.create_named(&format!("page-{index}"))
                .await
                .string("brandProfileId"),
        );
    }
    created.sort();

    // Each row is a page size: both published bounds of `maxResults` (1 and 100), an
    // interior size that divides the set, one that does not, and no `maxResults` at all.
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref next) = token {
                query.push(format!("nextToken={next}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let page = em.list(&query).await;
            assert_eq!(
                page.status, 200,
                "case {case}: ListBrandProfiles must succeed, got {} / {:?}",
                page.status, page.body
            );
            let items = page
                .member("brandProfiles")
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: brandProfiles is not a list"))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    items.len() <= size,
                    "case {case}: a page must not exceed maxResults, got {} items",
                    items.len()
                );
            }
            for item in &items {
                seen.push(
                    item["brandProfileId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: an item has no brandProfileId"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= PROFILES + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );

            match page.member("nextToken").as_str() {
                Some(next) => {
                    assert!(
                        !items.is_empty(),
                        "case {case}: a token must not be emitted beside an empty page"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, created,
            "case {case}: paging must yield every profile exactly once, in id order"
        );
    }
}

#[tokio::test]
async fn an_update_applies_only_the_members_it_carries() {
    let em = Emulator::new();

    // Each row is one update body; `expected` is (name, deletionProtectionEnabled)
    // after it is applied to a profile created as ("before", false).
    let cases: Vec<(Value, &str, bool)> = vec![
        (json!({}), "before", false),
        (json!({ "brandProfileName": "after" }), "after", false),
        (json!({ "deletionProtectionEnabled": true }), "before", true),
        (
            json!({ "brandProfileName": "both", "deletionProtectionEnabled": true }),
            "both",
            true,
        ),
    ];

    for (body, expected_name, expected_protection) in cases {
        let case = format!("update={body}");
        let created = em
            .create(json!({ "brandProfileName": "before", "deletionProtectionEnabled": false }))
            .await;
        let id = created.string("brandProfileId");

        let updated = em.update(&id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: UpdateBrandProfile must succeed, got {} / {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("brandProfileName"),
            expected_name,
            "case {case}: unexpected name after update, got {:?}",
            updated.body
        );
        assert_eq!(
            updated.member("deletionProtectionEnabled"),
            &json!(expected_protection),
            "case {case}: unexpected deletionProtectionEnabled after update, got {:?}",
            updated.body
        );
        assert_eq!(
            updated.number("createdAt"),
            created.number("createdAt"),
            "case {case}: an update must not move createdAt, got {:?}",
            updated.body
        );
        assert!(
            updated.number("updatedAt") > created.number("updatedAt"),
            "case {case}: an update must bump updatedAt, got {} then {}",
            created.number("updatedAt"),
            updated.number("updatedAt")
        );
        assert_eq!(
            updated.string("brandProfileArn"),
            created.string("brandProfileArn"),
            "case {case}: an update must not re-mint the ARN, got {:?}",
            updated.body
        );

        // The mutation is persisted, not just reported.
        let fetched = em.get(&id).await;
        for member in ["brandProfileName", "deletionProtectionEnabled", "updatedAt"] {
            assert_eq!(
                fetched.member(member),
                updated.member(member),
                "case {case}: GetBrandProfile must reflect the update's {member}, got {:?}",
                fetched.body
            );
        }

        // Leave the store clean for the next row.
        if expected_protection {
            em.update(&id, json!({ "deletionProtectionEnabled": false }))
                .await;
        }
        assert_eq!(
            em.delete(&id).await.status,
            200,
            "case {case}: tearing the profile down must succeed"
        );
    }
}

#[tokio::test]
async fn deletion_protection_refuses_the_delete_until_it_is_turned_off() {
    let em = Emulator::new();
    let created = em
        .create(json!({ "brandProfileName": "protected", "deletionProtectionEnabled": true }))
        .await;
    let id = created.string("brandProfileId");

    let refused = em.delete(&id).await;
    assert_eq!(
        refused.status, 409,
        "a protected profile's delete is ConflictException (409), got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ConflictException"),
        "unexpected error code, got {:?}",
        refused.error
    );
    assert_eq!(
        em.get(&id).await.status,
        200,
        "a refused delete must leave the profile in place"
    );

    assert_eq!(
        em.update(&id, json!({ "deletionProtectionEnabled": false }))
            .await
            .status,
        200,
        "turning protection off must succeed"
    );

    let deleted = em.delete(&id).await;
    assert_eq!(
        deleted.status, 200,
        "an unprotected profile deletes, got {} / {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("brandProfileId"),
        id,
        "delete must report the id it removed, got {:?}",
        deleted.body
    );
    assert_eq!(
        deleted.string("brandProfileArn"),
        created.string("brandProfileArn"),
        "delete must report the ARN it removed, got {:?}",
        deleted.body
    );

    assert_eq!(
        em.get(&id).await.status,
        404,
        "a deleted profile must not be gettable"
    );
    let listed = em.list("").await;
    assert_eq!(
        listed.member("brandProfiles"),
        &json!([]),
        "a deleted profile must not be listed, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn an_id_that_names_nothing_is_a_404_on_every_operation_that_takes_one() {
    let em = Emulator::new();
    // Rows: never minted, minted-looking, and an id belonging to a deleted profile.
    let deleted = em.create_named("gone").await.string("brandProfileId");
    assert_eq!(em.delete(&deleted).await.status, 200, "teardown must work");

    for reference in ["bp-nosuchprofile", "never_created", deleted.as_str()] {
        for (method, body) in [
            ("GET", None),
            ("PUT", Some(json!({ "brandProfileName": "x" }))),
            ("DELETE", None),
        ] {
            let case = format!("{method} {reference}");
            let reply = em
                .call(
                    method,
                    &format!("/v1/brand-profiles/{reference}"),
                    body.clone(),
                )
                .await;
            assert_eq!(
                reply.status, 404,
                "case {case}: expected 404, got {} / {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: unexpected error code, got {:?}",
                reply.error
            );
        }
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_a_400_validation_exception() {
    let em = Emulator::new();

    // `brandProfileName` is @length(1..64) with pattern
    // ^[A-Za-z0-9_ -]*[A-Za-z0-9_-][A-Za-z0-9_ -]*$: both bounds, one past the upper
    // bound, and a character the pattern excludes.
    let bad_names = vec![
        String::new(),
        " ".to_string(),
        "bad!name".to_string(),
        "no.dots".to_string(),
        "A".repeat(65),
    ];
    for name in bad_names {
        let reply = em.create(json!({ "brandProfileName": name })).await;
        assert_eq!(
            reply.status, 400,
            "case name={name:?}: expected 400, got {} / {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case name={name:?}: unexpected error code, got {:?}",
            reply.error
        );
    }

    // `maxResults` is @range(1..100): each published bound is accepted, each value
    // one step outside it is refused.
    for accepted in ["1", "100"] {
        let reply = em.list(&format!("?maxResults={accepted}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={accepted}: a published bound must be accepted, got {} / {:?}",
            reply.status, reply.body
        );
    }
    for refused in ["0", "101"] {
        let reply = em.list(&format!("?maxResults={refused}")).await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={refused}: expected 400, got {} / {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case maxResults={refused}: unexpected error code, got {:?}",
            reply.error
        );
    }

    // A `nextToken` this service never issued is refused rather than silently
    // treated as "start from the beginning" or as some other resume point. The rows
    // span the ways a fabricated token can fail: not hex at all, odd-length hex,
    // hex with a separator, valid hex that decodes to text carrying no token marker
    // ("6262" -> "bb"), and valid hex carrying a marker that is not this one.
    let unissued_tokens = vec![
        "notatoken".to_string(),
        "zz".to_string(),
        "6e6f-74-hex".to_string(),
        "6262".to_string(),
        hex::encode("bp-0123456789abcdef0"),
        hex::encode("SomeOtherOperation/1/bp-0123456789abcdef0"),
    ];
    for token in unissued_tokens {
        let reply = em.list(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case nextToken={token:?}: expected 400, got {} / {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case nextToken={token:?}: unexpected error code, got {:?}",
            reply.error
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_reports_the_first_profile() {
    let em = Emulator::new();

    // Rows differ in what the repeat asks for: the same body, a different name, and a
    // different deletionProtectionEnabled. An idempotent repeat answers the first
    // result in every one.
    let repeats: Vec<Value> = vec![
        json!({ "brandProfileName": "first" }),
        json!({ "brandProfileName": "second" }),
        json!({ "brandProfileName": "first", "deletionProtectionEnabled": true }),
    ];

    for (index, repeat) in repeats.into_iter().enumerate() {
        let token = format!("token-{index}");
        let case = format!("clientToken={token} repeat={repeat}");
        let first = em
            .create(json!({ "brandProfileName": "first", "clientToken": token }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case}: the first create must succeed, got {} / {:?}",
            first.status, first.body
        );

        let mut repeat_body = repeat;
        repeat_body["clientToken"] = json!(token);
        let again = em.create(repeat_body).await;
        assert_eq!(
            again.status, 200,
            "case {case}: the repeat must succeed, got {} / {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case {case}: a repeated clientToken must report the first result"
        );
    }

    // Three tokens, three profiles — the repeats created nothing extra.
    let listed = em.list("").await;
    assert_eq!(
        listed
            .member("brandProfiles")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        3,
        "a repeated clientToken must not create a second profile, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn the_store_is_bounded_and_refuses_a_create_past_its_ceiling() {
    use service_com_amazonaws_endusermessaging_endusermessaging::state::MAX_BRAND_PROFILES;

    let em = Emulator::new();
    for index in 0..MAX_BRAND_PROFILES {
        let reply = em.create_named(&format!("bounded-{index}")).await;
        assert_eq!(
            reply.status, 200,
            "case index={index}: a create below the ceiling must succeed, got {} / {:?}",
            reply.status, reply.body
        );
    }

    // Rows: a plain create, one with tags, and one with a client token. None of
    // them may grow the store past the ceiling.
    let bodies = vec![
        json!({ "brandProfileName": "one-too-many" }),
        json!({ "brandProfileName": "one-too-many", "tags": [{ "key": "k", "value": "v" }] }),
        json!({ "brandProfileName": "one-too-many", "clientToken": "fresh-token" }),
    ];
    for body in bodies {
        let case = format!("body={body}");
        let refused = em.create(body).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: a create at the ceiling is ServiceQuotaExceededException (400), \
             got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: unexpected error code, got {:?}",
            refused.error
        );
    }

    // The ceiling refused the extra creates rather than dropping anything already
    // stored, and paging still walks exactly the stored set.
    let mut counted = 0usize;
    let mut token: Option<String> = None;
    loop {
        let query = match token {
            Some(ref next) => format!("?maxResults=100&nextToken={next}"),
            None => "?maxResults=100".to_string(),
        };
        let page = em.list(&query).await;
        counted += page
            .member("brandProfiles")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        match page.member("nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        counted, MAX_BRAND_PROFILES,
        "the store must hold exactly its ceiling, got {counted}"
    );

    // Freeing a slot lets a create succeed again.
    let first = em.list("?maxResults=1").await;
    let victim = first.member("brandProfiles")[0]["brandProfileId"]
        .as_str()
        .expect("a stored profile to delete")
        .to_string();
    assert_eq!(
        em.delete(&victim).await.status,
        200,
        "deleting a stored profile must succeed"
    );
    let after = em.create_named("room-again").await;
    assert_eq!(
        after.status, 200,
        "a create must succeed once a slot is free, got {} / {:?}",
        after.status, after.body
    );
}

#[tokio::test]
async fn an_empty_store_lists_nothing_and_offers_no_token() {
    let em = Emulator::new();
    let listed = em.list("").await;
    assert_eq!(
        listed.status, 200,
        "an empty list is a 200, got {} / {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.member("brandProfiles"),
        &json!([]),
        "an empty store lists nothing, got {:?}",
        listed.body
    );
    assert!(
        listed.member("nextToken").is_null(),
        "an empty page offers no token, got {:?}",
        listed.body
    );
}
