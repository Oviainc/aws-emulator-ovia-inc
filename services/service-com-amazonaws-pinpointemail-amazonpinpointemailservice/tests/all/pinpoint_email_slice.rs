// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the implemented Pinpoint Email slice: the email-identity
//! lifecycle, configuration sets, and `SendEmail`.
//!
//! Every test drives the service the way a client does — through the multi-service
//! router, over `restJson1` requests signed for the `ses` signing name — so what is
//! asserted is the wire behaviour, not a handler's return value.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pinpointemail_amazonpinpointemailservice::state::{
    ComAmazonawsPinpointemailAmazonpinpointemailserviceState, ConfigurationSetRecord,
    EmailIdentityRecord, MAX_CONFIGURATION_SETS, MAX_EMAIL_IDENTITIES, SENT_MESSAGE_LOG_LIMIT,
    VERIFICATION_DUE_AFTER,
};
use service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService;

/// A SigV4 credential scope for signing name `ses` — the routing signal that makes the
/// router hand the request to this service (OPERATIONS.md, "Wire protocol"). The
/// signature itself is never checked: the test router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ses/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// One answer from the emulator: the HTTP status, the `x-amzn-errortype` error code when
/// the answer is a modelled failure, and the parsed JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body member `name`, or `Value::Null` when the response omits it.
    fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    /// A boolean body member, defaulting to the Smithy `@default false` the model
    /// declares for the `Enabled` members when the response omits it.
    fn flag(&self, name: &str) -> bool {
        self.member(name).as_bool().unwrap_or(false)
    }

    /// The body member `name` as a string.
    fn text(&self, name: &str) -> Option<&str> {
        self.member(name).as_str()
    }
}

/// One emulator: a router with only this service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the mounted service holds. A test reads or pre-fills the store
    /// through it — the way a fixture reaches state no implemented operation can write (a
    /// store already at its maximum, the sent-message log) — never to serve a request.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Run `body` against this emulator's store.
    fn with_store<T>(
        &self,
        body: impl FnOnce(&mut ComAmazonawsPinpointemailAmazonpinpointemailserviceState) -> T,
    ) -> T {
        let state = self
            .engine
            .state::<ComAmazonawsPinpointemailAmazonpinpointemailserviceState>();
        let mut guard = state.lock();
        body(&mut guard)
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| Bytes::from(value.to_string()))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{uri}"))
            .header(http::header::AUTHORIZATION, AUTHORIZATION)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(payload)
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
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
        Answer {
            status,
            error,
            body,
        }
    }

    async fn create_identity(&self, identity: &str, tags: Option<Value>) -> Answer {
        let mut body = json!({ "EmailIdentity": identity });
        if let Some(tags) = tags {
            body["Tags"] = tags;
        }
        self.call("POST", "/v1/email/identities", Some(body)).await
    }

    async fn get_identity(&self, identity: &str) -> Answer {
        self.call("GET", &format!("/v1/email/identities/{identity}"), None)
            .await
    }

    async fn delete_identity(&self, identity: &str) -> Answer {
        self.call("DELETE", &format!("/v1/email/identities/{identity}"), None)
            .await
    }

    async fn list_identities(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/v1/email/identities".to_string()
        } else {
            format!("/v1/email/identities?{query}")
        };
        self.call("GET", &uri, None).await
    }

    async fn create_configuration_set(&self, body: Value) -> Answer {
        self.call("POST", "/v1/email/configuration-sets", Some(body))
            .await
    }

    async fn get_configuration_set(&self, name: &str) -> Answer {
        self.call(
            "GET",
            &format!("/v1/email/configuration-sets/{name}"),
            None,
        )
        .await
    }

    async fn send_email(&self, body: Value) -> Answer {
        self.call("POST", "/v1/email/outbound-emails", Some(body))
            .await
    }
}

/// The names in an `EmailIdentities` list, in the order the response gave them.
fn listed_names(answer: &Answer) -> Vec<String> {
    answer
        .member("EmailIdentities")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("IdentityName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The DKIM tokens in a `DkimAttributes` object, in the order the response gave them.
fn dkim_tokens(dkim: &Value) -> Vec<String> {
    dkim.get("Tokens")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `IdentityType` follows the identity's own form: an address (a local part and a domain
/// separated by `@`) is an `EMAIL_ADDRESS`, anything else names a `DOMAIN`. A domain
/// identity also gets DKIM attributes, because DKIM verification is done over a domain's
/// DNS records; an address identity gets none.
///
/// The rows differ along the form of the identity, which is what the rule reads.
#[tokio::test]
async fn identity_type_and_dkim_follow_the_form_of_the_identity() {
    // `IdentityType` values: shared_types::IDENTITY_TYPE_VALUES (DOMAIN, EMAIL_ADDRESS,
    // MANAGED_DOMAIN); MANAGED_DOMAIN is a domain AWS itself manages, so no
    // CreateEmailIdentity call produces it.
    let cases: &[(&str, &str)] = &[
        ("example.com", "DOMAIN"),
        ("mail.marketing.example.co.uk", "DOMAIN"),
        ("xn--bcher-kva.example", "DOMAIN"),
        ("sender@example.com", "EMAIL_ADDRESS"),
        ("first.last+campaign@mail.example.org", "EMAIL_ADDRESS"),
    ];

    let emulator = Emulator::new();
    let mut all_tokens: BTreeSet<String> = BTreeSet::new();

    for (identity, expected_type) in cases {
        let created = emulator.create_identity(identity, None).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected HTTP 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("IdentityType"),
            Some(*expected_type),
            "case {identity:?}: expected IdentityType {expected_type}, got {:?}",
            created.body
        );
        assert!(
            !created.flag("VerifiedForSendingStatus"),
            "case {identity:?}: a fresh identity is not verified yet, got {:?}",
            created.body
        );

        let dkim = created.member("DkimAttributes").clone();
        if *expected_type == "DOMAIN" {
            assert_eq!(
                dkim.get("Status").and_then(Value::as_str),
                Some("PENDING"),
                "case {identity:?}: expected DkimAttributes.Status PENDING, got {dkim:?}"
            );
            assert_eq!(
                dkim.get("SigningEnabled").and_then(Value::as_bool),
                Some(true),
                "case {identity:?}: expected DkimAttributes.SigningEnabled true, got {dkim:?}"
            );
            let tokens = dkim_tokens(&dkim);
            assert_eq!(
                tokens.len(),
                3,
                "case {identity:?}: expected three DKIM tokens, got {tokens:?}"
            );
            for token in &tokens {
                assert!(
                    token.len() == 32 && token.chars().all(|c| c.is_ascii_alphanumeric()),
                    "case {identity:?}: expected a 32-character alphanumeric DKIM token, \
                     got {token:?}"
                );
                assert!(
                    all_tokens.insert(token.clone()),
                    "case {identity:?}: DKIM token {token:?} was already handed to another \
                     identity"
                );
            }
        } else {
            assert!(
                dkim.is_null(),
                "case {identity:?}: an address identity has no DKIM attributes, got {dkim:?}"
            );
        }

        // Round trip: the read reports exactly what the create persisted.
        let fetched = emulator.get_identity(identity).await;
        assert_eq!(
            fetched.status, 200,
            "case {identity:?}: expected GetEmailIdentity 200, got {} {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.text("IdentityType"),
            Some(*expected_type),
            "case {identity:?}: GetEmailIdentity must report the persisted IdentityType, \
             got {:?}",
            fetched.body
        );
        assert_eq!(
            dkim_tokens(fetched.member("DkimAttributes")),
            dkim_tokens(&dkim),
            "case {identity:?}: GetEmailIdentity must report the persisted DKIM tokens, \
             got {:?}",
            fetched.body
        );
        assert!(
            fetched.flag("FeedbackForwardingStatus"),
            "case {identity:?}: feedback forwarding is on for a new identity, got {:?}",
            fetched.body
        );
    }

    let listed = emulator.list_identities("").await;
    let mut expected: Vec<String> = cases.iter().map(|(name, _)| (*name).to_string()).collect();
    expected.sort();
    assert_eq!(
        listed_names(&listed),
        expected,
        "every created identity must be listed exactly once, got {:?}",
        listed.body
    );
}

/// Verification is a transition the emulator's clock owes: a new identity is reported
/// unverified with DKIM `PENDING` straight after the create, and once
/// [`VERIFICATION_DUE_AFTER`] has passed every read of it \u2014 `GetEmailIdentity`,
/// `ListEmailIdentities`, a repeat `CreateEmailIdentity` \u2014 reports it verified and able to
/// send, with a domain's DKIM status at `SUCCESS`.
///
/// The rows differ along the identity's form, because an address verifies the same way a
/// domain does while carrying no DKIM attributes.
#[tokio::test]
async fn identity_verification_comes_due_on_the_clock() {
    let emulator = Emulator::new();
    let cases = ["example.com", "sender@example.com"];
    for identity in cases {
        emulator.create_identity(identity, None).await;

        // The span is never zero, so a caller acting straight after the create still sees
        // the pending identity.
        let pending = emulator.get_identity(identity).await;
        assert!(
            !pending.flag("VerifiedForSendingStatus"),
            "case {identity:?}: a just-created identity is not verified yet, got {:?}",
            pending.body
        );
        if identity.contains('@') {
            assert!(
                pending.member("DkimAttributes").is_null(),
                "case {identity:?}: an address identity has no DKIM attributes, got {:?}",
                pending.body
            );
        } else {
            assert_eq!(
                pending
                    .member("DkimAttributes")
                    .get("Status")
                    .and_then(Value::as_str),
                Some("PENDING"),
                "case {identity:?}: DKIM is pending until verification comes due, got {:?}",
                pending.body
            );
        }
    }

    tokio::time::sleep(VERIFICATION_DUE_AFTER + std::time::Duration::from_millis(250)).await;

    for identity in cases {
        let verified = emulator.get_identity(identity).await;
        assert!(
            verified.flag("VerifiedForSendingStatus"),
            "case {identity:?}: verification was due, so the identity reports verified, \
             got {:?}",
            verified.body
        );
        if !identity.contains('@') {
            assert_eq!(
                verified
                    .member("DkimAttributes")
                    .get("Status")
                    .and_then(Value::as_str),
                Some("SUCCESS"),
                "case {identity:?}: expected DkimAttributes.Status SUCCESS, got {:?}",
                verified.body
            );
        }

        let repeated = emulator.create_identity(identity, None).await;
        assert!(
            repeated.flag("VerifiedForSendingStatus"),
            "case {identity:?}: a repeat create reports the verified record, got {:?}",
            repeated.body
        );
    }

    let listed = emulator.list_identities("").await;
    let sending: Vec<(String, bool)> = listed
        .member("EmailIdentities")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.get("IdentityName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("SendingEnabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for (name, sending_enabled) in &sending {
        assert!(
            sending_enabled,
            "case {name:?}: a verified identity is listed with SendingEnabled true, got {:?}",
            listed.body
        );
    }
    assert_eq!(
        sending.len(),
        cases.len(),
        "every identity must be listed, got {:?}",
        listed.body
    );
}

/// Verifying an identity that already exists keeps the single record: the tags the repeat
/// call carries are applied, everything the first call minted survives, and the identity
/// is still listed once.
#[tokio::test]
async fn creating_the_same_identity_twice_keeps_one_record() {
    let emulator = Emulator::new();

    for identity in ["example.com", "sender@example.net"] {
        let first = emulator
            .create_identity(
                identity,
                Some(json!([{ "Key": "team", "Value": "marketing" }])),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {identity:?}: expected HTTP 200, got {} {:?}",
            first.status, first.body
        );

        let second = emulator
            .create_identity(identity, Some(json!([{ "Key": "team", "Value": "support" }])))
            .await;
        assert_eq!(
            second.status, 200,
            "case {identity:?}: a repeat create succeeds, got {} {:?}",
            second.status, second.body
        );
        assert_eq!(
            dkim_tokens(second.member("DkimAttributes")),
            dkim_tokens(first.member("DkimAttributes")),
            "case {identity:?}: a repeat create must not re-mint the DKIM tokens, got {:?}",
            second.body
        );

        let fetched = emulator.get_identity(identity).await;
        assert_eq!(
            fetched.member("Tags"),
            &json!([{ "Key": "team", "Value": "support" }]),
            "case {identity:?}: the repeat create's tags must be persisted, got {:?}",
            fetched.body
        );

        let listed = emulator.list_identities("").await;
        let occurrences = listed_names(&listed)
            .iter()
            .filter(|name| name.as_str() == identity)
            .count();
        assert_eq!(
            occurrences, 1,
            "case {identity:?}: expected exactly one list entry, got {:?}",
            listed.body
        );
    }
}

/// An operation that names an identity the store does not hold answers
/// `NotFoundException` (HTTP 404) — not an empty success.
#[tokio::test]
async fn an_unknown_identity_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_identity("example.com", None).await;

    let cases = ["absent.example", "nobody@example.com", "example.com.evil"];
    for identity in cases {
        for (operation, answer) in [
            ("GetEmailIdentity", emulator.get_identity(identity).await),
            (
                "DeleteEmailIdentity",
                emulator.delete_identity(identity).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {operation} {identity:?}: expected HTTP 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("NotFoundException"),
                "case {operation} {identity:?}: expected NotFoundException, got {:?}",
                answer.error
            );
        }
    }
}

/// Deleting an identity removes the record: the read 404s and the list no longer carries
/// it, while every other identity is untouched.
#[tokio::test]
async fn deleting_an_identity_removes_only_that_record() {
    let emulator = Emulator::new();
    let identities = ["a.example", "b.example", "user@c.example"];
    for identity in identities {
        emulator.create_identity(identity, None).await;
    }

    for deleted in identities {
        let emulator = Emulator::new();
        for identity in identities {
            emulator.create_identity(identity, None).await;
        }

        let answer = emulator.delete_identity(deleted).await;
        assert_eq!(
            answer.status, 200,
            "case {deleted:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );

        let fetched = emulator.get_identity(deleted).await;
        assert_eq!(
            fetched.status, 404,
            "case {deleted:?}: a deleted identity must be gone, got {} {:?}",
            fetched.status, fetched.body
        );

        let listed = emulator.list_identities("").await;
        let mut expected: Vec<String> = identities
            .iter()
            .filter(|name| **name != deleted)
            .map(|name| (*name).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {deleted:?}: only the deleted identity disappears, got {:?}",
            listed.body
        );
    }
}

/// `ListEmailIdentities` pages: a page holds at most `PageSize` identities, a `NextToken`
/// comes back exactly while identities remain, and feeding that token back returns the
/// identities that follow — so paging with any page size visits every identity once and
/// then stops.
///
/// The rows differ along the page size, including one that divides the identity count
/// evenly and one larger than the whole list.
#[tokio::test]
async fn paging_visits_every_identity_once_for_any_page_size() {
    let identities = [
        "a.example",
        "b.example",
        "c.example",
        "d@d.example",
        "e.example",
        "f.example",
    ];
    let mut expected: Vec<String> = identities.iter().map(|n| (*n).to_string()).collect();
    expected.sort();

    for page_size in [1_usize, 2, 3, 5, 6, 7, 1000] {
        let emulator = Emulator::new();
        for identity in identities {
            emulator.create_identity(identity, None).await;
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= identities.len() + 1,
                "case PageSize={page_size}: paging did not terminate, saw {seen:?}"
            );
            let query = match &token {
                Some(token) => format!("PageSize={page_size}&NextToken={token}"),
                None => format!("PageSize={page_size}"),
            };
            let answer = emulator.list_identities(&query).await;
            assert_eq!(
                answer.status, 200,
                "case PageSize={page_size}: expected HTTP 200, got {} {:?}",
                answer.status, answer.body
            );
            let page = listed_names(&answer);
            assert!(
                page.len() <= page_size,
                "case PageSize={page_size}: a page held {} identities: {page:?}",
                page.len()
            );
            seen.extend(page);
            match answer.text("NextToken") {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case PageSize={page_size}: paging must visit every identity once, in the \
             store's order"
        );
    }
}

/// `PageSize` has to be at least 0 and no more than 1000
/// (`ListEmailIdentitiesRequest$PageSize`), and `NextToken` has to be a token this
/// service minted: a value outside that is refused with `BadRequestException` (HTTP 400),
/// never served as a page.
#[tokio::test]
async fn a_page_request_outside_the_documented_bounds_is_refused() {
    let emulator = Emulator::new();
    for identity in ["a.example", "b.example"] {
        emulator.create_identity(identity, None).await;
    }

    // Each published bound, and one step past each of them.
    for (query, expected_status) in [
        ("PageSize=0", 200),
        ("PageSize=1000", 200),
        ("PageSize=-1", 400),
        ("PageSize=1001", 400),
        ("NextToken=not-a-token", 400),
        ("NextToken=ff", 400),
    ] {
        let answer = emulator.list_identities(query).await;
        assert_eq!(
            answer.status, expected_status,
            "case {query:?}: expected HTTP {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error.as_deref(),
                Some("BadRequestException"),
                "case {query:?}: expected BadRequestException, got {:?}",
                answer.error
            );
        }
    }

    // A page of zero identities returns nothing and no token, so a client looping on
    // NextToken terminates instead of asking for the same empty page for ever.
    let empty = emulator.list_identities("PageSize=0").await;
    assert!(
        listed_names(&empty).is_empty(),
        "case PageSize=0: expected an empty page, got {:?}",
        empty.body
    );
    assert!(
        empty.text("NextToken").is_none(),
        "case PageSize=0: an empty page carries no NextToken, got {:?}",
        empty.body
    );
}

/// Each store this slice writes holds a bounded number of records: a create that would
/// grow one past its maximum is refused with `LimitExceededException` (HTTP 400), while a
/// create at one under the maximum still succeeds and a repeat create of an identity that
/// already exists is accepted even at the cap, because it grows nothing.
#[tokio::test]
async fn a_create_past_a_store_maximum_is_refused() {
    let emulator = Emulator::new();
    let now = std::time::SystemTime::now();
    // Fixture: fill both stores to one under their maximum directly, since reaching the
    // cap through the API would mean tens of thousands of requests.
    emulator.with_store(|store| {
        for index in 0..MAX_EMAIL_IDENTITIES - 1 {
            let name = format!("filler-{index}.example");
            store
                .identities
                .insert(name.clone(), EmailIdentityRecord::new(&name, now));
        }
        for index in 0..MAX_CONFIGURATION_SETS - 1 {
            let name = format!("filler-set-{index}");
            store.configuration_sets.insert(
                name.clone(),
                ConfigurationSetRecord {
                    name,
                    ..ConfigurationSetRecord::default()
                },
            );
        }
    });

    // The last record under each maximum is accepted.
    let last_identity = emulator.create_identity("last.example", None).await;
    assert_eq!(
        last_identity.status, 200,
        "the identity at the maximum is accepted, got {} {:?}",
        last_identity.status, last_identity.body
    );
    let last_set = emulator
        .create_configuration_set(json!({ "ConfigurationSetName": "last-set" }))
        .await;
    assert_eq!(
        last_set.status, 200,
        "the configuration set at the maximum is accepted, got {} {:?}",
        last_set.status, last_set.body
    );

    // One past each maximum is refused.
    for (case, answer) in [
        (
            "CreateEmailIdentity",
            emulator.create_identity("one-too-many.example", None).await,
        ),
        (
            "CreateConfigurationSet",
            emulator
                .create_configuration_set(json!({ "ConfigurationSetName": "one-too-many" }))
                .await,
        ),
    ] {
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("LimitExceededException"),
            "case {case}: expected LimitExceededException, got {:?}",
            answer.error
        );
    }

    // A repeat create of an identity that already exists grows nothing, so the cap does
    // not refuse it.
    let repeat = emulator.create_identity("last.example", None).await;
    assert_eq!(
        repeat.status, 200,
        "a repeat create at the maximum is accepted, got {} {:?}",
        repeat.status, repeat.body
    );
    emulator.with_store(|store| {
        assert_eq!(
            store.identities.len(),
            MAX_EMAIL_IDENTITIES,
            "the store must hold exactly its maximum"
        );
        assert_eq!(
            store.configuration_sets.len(),
            MAX_CONFIGURATION_SETS,
            "the store must hold exactly its maximum"
        );
    });
}

/// The sent-message log is bounded: it retains the most recent
/// `SENT_MESSAGE_LOG_LIMIT` messages and drops the oldest beyond that, and every send
/// still gets a `MessageId` of its own, because the id counter is independent of the log.
#[tokio::test]
async fn the_sent_message_log_retains_a_bounded_number_of_messages() {
    let emulator = Emulator::new();
    let sends = SENT_MESSAGE_LOG_LIMIT + 5;
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for index in 0..sends {
        let answer = emulator
            .send_email(json!({
                "Destination": { "ToAddresses": [format!("dest-{index}@example.org")] },
                "Content": { "Simple": {
                    "Subject": { "Data": format!("message {index}") },
                    "Body": { "Text": { "Data": "a body" } },
                }},
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case send {index}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let id = answer.text("MessageId").unwrap_or_default().to_string();
        assert!(
            ids.insert(id.clone()),
            "case send {index}: MessageId {id:?} repeated an earlier send's id"
        );
    }

    assert_eq!(
        ids.len(),
        sends,
        "every send must get its own MessageId, even past the log's bound"
    );
    emulator.with_store(|store| {
        assert_eq!(
            store.sent_messages.len(),
            SENT_MESSAGE_LOG_LIMIT,
            "the log must retain exactly its bound, not every message ever sent"
        );
        let oldest = store
            .sent_messages
            .front()
            .expect("the log retains messages");
        assert_eq!(
            oldest.content_subject.as_deref(),
            Some(format!("message {}", sends - SENT_MESSAGE_LOG_LIMIT).as_str()),
            "the log must drop its OLDEST entries, got {oldest:?}"
        );
    });
}

/// A configuration set round-trips: `GetConfigurationSet` reports the name and every
/// option structure and tag `CreateConfigurationSet` persisted, and an option the create
/// did not carry is reported by neither.
///
/// The rows differ along which of the four option structures the create carries.
#[tokio::test]
async fn a_configuration_set_round_trips_through_the_store() {
    let cases: &[(&str, Value)] = &[
        ("bare-set", json!({})),
        (
            "sending-set",
            json!({ "SendingOptions": { "SendingEnabled": true } }),
        ),
        (
            "tracking-set",
            json!({ "TrackingOptions": { "CustomRedirectDomain": "click.example.com" } }),
        ),
        (
            "delivery-set",
            json!({ "DeliveryOptions": { "TlsPolicy": "REQUIRE", "SendingPoolName": "pool-1" } }),
        ),
        (
            "full-set",
            json!({
                "TrackingOptions": { "CustomRedirectDomain": "click.example.org" },
                "DeliveryOptions": { "TlsPolicy": "OPTIONAL", "SendingPoolName": "pool-2" },
                "ReputationOptions": { "ReputationMetricsEnabled": true },
                "SendingOptions": { "SendingEnabled": false },
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (name, options) in cases {
        let mut body = json!({ "ConfigurationSetName": name });
        for (member, value) in options.as_object().expect("the case options are an object") {
            body[member] = value.clone();
        }

        let created = emulator.create_configuration_set(body).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200, got {} {:?}",
            created.status, created.body
        );

        let fetched = emulator.get_configuration_set(name).await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: expected GetConfigurationSet 200, got {} {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.text("ConfigurationSetName"),
            Some(*name),
            "case {name:?}: the stored name must come back, got {:?}",
            fetched.body
        );
        for member in [
            "TrackingOptions",
            "DeliveryOptions",
            "ReputationOptions",
            "SendingOptions",
            "Tags",
        ] {
            let expected = options.get(member).cloned().unwrap_or(Value::Null);
            // `SendingOptions.SendingEnabled: false` is the member's Smithy default, so
            // the response may omit it; anything else must come back as it went in.
            if expected == json!({ "SendingEnabled": false }) {
                continue;
            }
            assert_eq!(
                fetched.member(member),
                &expected,
                "case {name:?}: expected {member} {expected:?}, got {:?}",
                fetched.member(member)
            );
        }
    }

    // A name already in use is refused, and the stored set is left alone.
    let duplicate = emulator
        .create_configuration_set(json!({ "ConfigurationSetName": "full-set" }))
        .await;
    assert_eq!(
        duplicate.status, 400,
        "a duplicate configuration set name is refused, got {} {:?}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error.as_deref(),
        Some("AlreadyExistsException"),
        "expected AlreadyExistsException, got {:?}",
        duplicate.error
    );
    let fetched = emulator.get_configuration_set("full-set").await;
    assert_eq!(
        fetched.text("ConfigurationSetName"),
        Some("full-set"),
        "the refused duplicate must not disturb the stored set, got {:?}",
        fetched.body
    );

    // A set that was never created is not found.
    for name in ["no-such-set", "full-set-2"] {
        let answer = emulator.get_configuration_set(name).await;
        assert_eq!(
            answer.status, 404,
            "case {name:?}: expected HTTP 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: expected NotFoundException, got {:?}",
            answer.error
        );
    }
}

/// A message with a recipient and content is accepted and gets its own `MessageId`: two
/// sends never share one.
///
/// The rows differ along the form of the content and the recipient fields used.
#[tokio::test]
async fn every_accepted_send_gets_its_own_message_id() {
    let emulator = Emulator::new();
    emulator
        .create_configuration_set(json!({ "ConfigurationSetName": "bootstrap-set" }))
        .await;

    let raw = aws_smithy_types::base64::encode(
        "Subject: raw\r\nFrom: sender@example.com\r\n\r\nbody\r\n",
    );
    let cases: &[(&str, Value)] = &[
        (
            "simple text to one recipient",
            json!({
                "FromEmailAddress": "sender@example.com",
                "Destination": { "ToAddresses": ["dest@example.org"] },
                "Content": { "Simple": {
                    "Subject": { "Data": "hello" },
                    "Body": { "Text": { "Data": "a plain body" } },
                }},
            }),
        ),
        (
            "simple html with a configuration set and tags",
            json!({
                "FromEmailAddress": "sender@example.com",
                "Destination": { "ToAddresses": ["one@example.org", "two@example.org"] },
                "Content": { "Simple": {
                    "Subject": { "Data": "hello again" },
                    "Body": { "Html": { "Data": "<p>a body</p>" } },
                }},
                "ConfigurationSetName": "bootstrap-set",
                "EmailTags": [{ "Name": "campaign", "Value": "launch" }],
                "ReplyToAddresses": ["replies@example.com"],
            }),
        ),
        (
            "bcc only",
            json!({
                "Destination": { "BccAddresses": ["hidden@example.org"] },
                "Content": { "Simple": {
                    "Subject": { "Data": "bcc" },
                    "Body": { "Text": { "Data": "body" } },
                }},
            }),
        ),
        (
            "cc only",
            json!({
                "Destination": { "CcAddresses": ["copy@example.org"] },
                "Content": { "Simple": {
                    "Subject": { "Data": "cc" },
                    "Body": { "Text": { "Data": "body" } },
                }},
            }),
        ),
        (
            "raw message",
            json!({
                "Destination": { "ToAddresses": ["dest@example.org"] },
                "Content": { "Raw": { "Data": raw } },
            }),
        ),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, body) in cases {
        let answer = emulator.send_email(body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );
        let message_id = answer
            .text("MessageId")
            .unwrap_or_default()
            .to_string();
        assert!(
            !message_id.is_empty(),
            "case {case:?}: expected a MessageId, got {:?}",
            answer.body
        );
        assert!(
            ids.insert(message_id.clone()),
            "case {case:?}: MessageId {message_id:?} was already returned by an earlier send"
        );
    }

    // The same request sent twice is still two messages with two ids.
    let repeated = cases[0].1.clone();
    let first = emulator.send_email(repeated.clone()).await;
    let second = emulator.send_email(repeated).await;
    assert_ne!(
        first.text("MessageId"),
        second.text("MessageId"),
        "two sends must not share a MessageId, got {:?} and {:?}",
        first.body,
        second.body
    );
}

/// `SendEmail` answers from persisted state rather than always succeeding: a
/// configuration set that was never created is `NotFoundException` (404), and a request
/// naming no recipient or carrying no content is `BadRequestException` (400).
#[tokio::test]
async fn send_email_refuses_a_message_it_cannot_send() {
    let emulator = Emulator::new();
    emulator
        .create_configuration_set(json!({ "ConfigurationSetName": "bootstrap-set" }))
        .await;

    let content = json!({ "Simple": {
        "Subject": { "Data": "hello" },
        "Body": { "Text": { "Data": "a body" } },
    }});
    let recipient = json!({ "ToAddresses": ["dest@example.org"] });

    let cases: &[(&str, Value, u16, &str)] = &[
        (
            "unknown configuration set",
            json!({
                "Destination": recipient,
                "Content": content,
                "ConfigurationSetName": "no-such-set",
            }),
            404,
            "NotFoundException",
        ),
        (
            "empty destination",
            json!({ "Destination": {}, "Content": content }),
            400,
            "BadRequestException",
        ),
        (
            "every address list empty",
            json!({
                "Destination": { "ToAddresses": [], "CcAddresses": [], "BccAddresses": [] },
                "Content": content,
            }),
            400,
            "BadRequestException",
        ),
        (
            "no content member",
            json!({ "Destination": recipient, "Content": {} }),
            400,
            "BadRequestException",
        ),
        (
            "simple message with neither subject nor body",
            json!({
                "Destination": recipient,
                "Content": { "Simple": { "Subject": { "Data": "" }, "Body": {} } },
            }),
            400,
            "BadRequestException",
        ),
        (
            "raw message with an empty payload",
            json!({
                "Destination": recipient,
                "Content": { "Raw": { "Data": "" } },
            }),
            400,
            "BadRequestException",
        ),
    ];

    for (case, body, expected_status, expected_error) in cases {
        let answer = emulator.send_email(body.clone()).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some(*expected_error),
            "case {case:?}: expected {expected_error}, got {:?}",
            answer.error
        );
        assert!(
            answer.text("MessageId").is_none(),
            "case {case:?}: a refused send mints no MessageId, got {:?}",
            answer.body
        );
    }

    // A refused send leaves the next good one working, and that one is accepted.
    let accepted = emulator
        .send_email(json!({
            "Destination": { "ToAddresses": ["dest@example.org"] },
            "Content": content,
            "ConfigurationSetName": "bootstrap-set",
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "a good send after a refused one is accepted, got {} {:?}",
        accepted.status, accepted.body
    );
}
