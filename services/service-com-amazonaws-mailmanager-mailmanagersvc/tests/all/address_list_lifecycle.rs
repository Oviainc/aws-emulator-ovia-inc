// SPDX-License-Identifier: CC0-1.0

//! The address-list lifecycle, end to end over the wire.
//!
//! Every assertion here goes through the real request path — `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>` — so
//! what it pins is the emulator's HTTP behaviour, not an internal helper.
//!
//! The tests are tables: each row is one input drawn from the class the rule covers
//! (several `AddressListName` shapes across the modelled pattern and both length bounds,
//! several tag sets, a name whose only difference is its letter case, and the
//! address-list quota).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::state::MAX_ADDRESS_LISTS;
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the ARNs the service returns must carry it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential
        // below is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field
            // is this service's signing name (`ses`) and whose 3rd is the region the
            // response's ARNs must use.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an address list and return its `AddressListId`, failing the test on any
    /// refusal.
    async fn create(&self, body: Value) -> String {
        let (status, out) = self.call("CreateAddressList", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body:?}: expected CreateAddressList to answer 200, got {status} {out:?}"
        );
        out["AddressListId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {body:?}: CreateAddressList returned no AddressListId: {out:?}")
            })
            .to_string()
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// A minted `AddressListId` satisfies `AddressListId`'s modelled constraints: length
/// 1..255 and pattern `^[a-zA-Z0-9-]+$`
/// (`smithy_query(query="shape",shape="AddressListId")`).
fn assert_well_formed_id(case: &str, id: &str) {
    assert!(
        (1..=255).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: AddressListId must match ^[a-zA-Z0-9-]+$ within 1..255, got {id:?}"
    );
}

/// Every created address list is readable back, exactly as it was stored, whatever the
/// shape of its name and whatever tags it carries.
///
/// The names span `AddressListName`'s modelled pattern `^[a-zA-Z0-9_.-]+$` and its length
/// bounds 1..255 (`smithy_query(query="shape",shape="AddressListName")`): the shortest
/// name the pattern admits, each punctuation character it allows, digits only, mixed case,
/// and the longest name it admits.
#[tokio::test]
async fn create_then_get_round_trips_every_name_shape() {
    let harness = Harness::new();

    let names: [String; 7] = [
        "a".to_string(),
        "with_underscore".to_string(),
        "with.dots.in.it".to_string(),
        "with-hyphens-too".to_string(),
        "0123456789".to_string(),
        "MixedCase_Name.1-x".to_string(),
        "x".repeat(255),
    ];
    // Rows also vary the tag set, which the record stores: none at all, one pair, and
    // several.
    let tag_sets: [Value; 3] = [
        Value::Null,
        json!([{ "Key": "team", "Value": "mail" }]),
        json!([
            { "Key": "team", "Value": "mail" },
            { "Key": "env", "Value": "test" },
            { "Key": "empty-value", "Value": "" },
        ]),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut body = json!({ "AddressListName": name });
        let tags = &tag_sets[index % tag_sets.len()];
        if !tags.is_null() {
            body["Tags"] = tags.clone();
        }
        let case = format!("name {name:?} with tags {tags:?}");

        let address_list_id = harness.create(body).await;
        assert_well_formed_id(&case, &address_list_id);
        minted.push(address_list_id.clone());

        let (status, out) = harness
            .call("GetAddressList", json!({ "AddressListId": address_list_id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetAddressList to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out["AddressListId"].as_str(),
            Some(address_list_id.as_str()),
            "case {case:?}: GetAddressList must report the id it was asked for, got {out:?}"
        );
        assert_eq!(
            out["AddressListName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: GetAddressList must report the stored name, got {out:?}"
        );
        assert_eq!(
            out["AddressListArn"].as_str(),
            Some(
                format!("arn:aws:ses:{REGION}:{ACCOUNT}:mailmanager-address-list/{address_list_id}")
                    .as_str()
            ),
            "case {case:?}: unexpected AddressListArn, got {out:?}"
        );
        let created = out["CreatedTimestamp"].as_f64();
        let updated = out["LastUpdatedTimestamp"].as_f64();
        assert!(
            created.is_some_and(|t| t > 0.0),
            "case {case:?}: CreatedTimestamp must be a real instant, got {out:?}"
        );
        assert_eq!(
            created, updated,
            "case {case:?}: a never-updated address list reports its creation instant as \
             LastUpdatedTimestamp, got {out:?}"
        );

        // A read changes nothing a later caller sees: a second read answers the same.
        let (_, again) = harness
            .call(
                "GetAddressList",
                json!({ "AddressListId": out["AddressListId"].as_str().unwrap_or_default() }),
            )
            .await;
        assert_eq!(
            again, out,
            "case {case:?}: GetAddressList must be stable across reads, got {again:?}"
        );
    }

    // Distinct creates mint distinct ids, even within one clock tick.
    let unique: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        unique.len(),
        minted.len(),
        "case mint-uniqueness: every create must mint a fresh id, got {minted:?}"
    );
}

/// An `AddressListName` already in use is a `ConflictException` (409), whatever the name's
/// shape; a repeat with the SAME `ClientToken` is a retry and returns the original id.
#[tokio::test]
async fn address_list_names_are_unique_and_client_tokens_are_idempotent() {
    let harness = Harness::new();

    let names: [String; 4] = [
        "a".to_string(),
        "dup_name.1".to_string(),
        "DUP-NAME-2".to_string(),
        "z".repeat(255),
    ];

    for (index, name) in names.iter().enumerate() {
        let token = format!("tok-{index}");
        let first = harness
            .create(json!({ "AddressListName": name, "ClientToken": token }))
            .await;

        // Same token, same request: a retry, not a second address list.
        let repeat = harness
            .create(json!({ "AddressListName": name, "ClientToken": token }))
            .await;
        assert_eq!(
            repeat, first,
            "case {name:?}: a retry with ClientToken {token:?} must return the original id"
        );

        // Fresh token, same name: a duplicate.
        let (status, out) = harness
            .call(
                "CreateAddressList",
                json!({ "AddressListName": name, "ClientToken": format!("fresh-{index}") }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected a duplicate name to answer 409, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {name:?}: unexpected error code, got {out:?}"
        );

        // The address list that holds the name is untouched by the refusal.
        let (status, held) = harness
            .call("GetAddressList", json!({ "AddressListId": first }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the original address list must survive a refused duplicate, got \
             {status} {held:?}"
        );
        assert_eq!(
            held["AddressListName"].as_str(),
            Some(name.as_str()),
            "case {name:?}: the original address list must keep its name, got {held:?}"
        );
    }

    // A token is a retry of ONE request: presented with a different request it is not a
    // retry, and must not be answered with the first request's address list. Rows vary
    // which member differs.
    let differing: [(&str, Value); 2] = [
        (
            "different name",
            json!({ "AddressListName": "token-reuse-other", "ClientToken": "tok-0" }),
        ),
        (
            "different tags",
            json!({
                "AddressListName": "a",
                "ClientToken": "tok-0",
                "Tags": [{ "Key": "team", "Value": "mail" }],
            }),
        ),
    ];
    for (case, body) in differing {
        let (status, out) = harness.call("CreateAddressList", body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a ClientToken for a different request must be refused, got \
             {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ConflictException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }
}

/// `AddressListName` is case-SENSITIVE: names differing only in letter case are different
/// address lists, because the modelled pattern admits both cases and nothing in the model
/// folds them.
#[tokio::test]
async fn names_differing_only_in_case_are_distinct_address_lists() {
    let harness = Harness::new();

    let cases: [(&str, &str); 3] = [
        ("lower vs upper", "allowlist"),
        ("mixed", "Allow.List-1"),
        ("single letter", "q"),
    ];
    for (case, name) in cases {
        let lowered = harness
            .create(json!({ "AddressListName": name.to_lowercase() }))
            .await;
        let uppered = harness
            .create(json!({ "AddressListName": name.to_uppercase() }))
            .await;
        assert_ne!(
            lowered, uppered,
            "case {case:?}: {name:?} in two letter cases must be two address lists"
        );

        let (_, out) = harness
            .call("GetAddressList", json!({ "AddressListId": uppered }))
            .await;
        assert_eq!(
            out["AddressListName"].as_str(),
            Some(name.to_uppercase().as_str()),
            "case {case:?}: the stored name must keep the casing it was created with, got {out:?}"
        );
    }
}

/// An `AddressListId` nothing created is refused at the status this service answers an id
/// it cannot resolve with (400), carrying the `ResourceNotFoundException` code and a message
/// naming the id the request asked for, and the refusal creates nothing.
#[tokio::test]
async fn get_address_list_on_an_unknown_id_is_refused() {
    let harness = Harness::new();
    let live = harness.create(json!({ "AddressListName": "present" })).await;

    // Rows: ids that satisfy `AddressListId`'s pattern but name nothing — the shortest
    // one, a hyphenated one, a long one, and the live id with one character changed.
    let unknown: [String; 4] = [
        "x".to_string(),
        "no-such-address-list".to_string(),
        "9".repeat(255),
        format!("{}0", &live[..live.len() - 1]),
    ];
    for id in &unknown {
        if id == &live {
            continue;
        }
        let (status, out) = harness
            .call("GetAddressList", json!({ "AddressListId": id }))
            .await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected an unknown AddressListId to answer 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {id:?}: unexpected error code, got {out:?}"
        );
        assert_eq!(
            out["Message"].as_str(),
            Some(format!("Could not find AddressList with id {id}").as_str()),
            "case {id:?}: unexpected refusal message, got {out:?}"
        );
    }

    let (status, out) = harness
        .call("GetAddressList", json!({ "AddressListId": live }))
        .await;
    assert_eq!(
        status, 200,
        "case live-after-misses: the stored address list must still be readable, got {status} \
         {out:?}"
    );
}

/// An `AddressListName` outside the modelled constraints is refused with a 400
/// `ValidationException` before anything is stored — never a 501 and never a create.
#[tokio::test]
async fn names_outside_the_modelled_constraints_are_refused() {
    let harness = Harness::new();

    // Rows walk each published bound of `AddressListName` (len 1..255, pattern
    // `^[a-zA-Z0-9_.-]+$`) and one step past it.
    let cases: [(&str, Value); 4] = [
        ("empty name, one below the minimum length", json!("")),
        (
            "256 characters, one past the maximum length",
            json!("x".repeat(256)),
        ),
        ("a space, outside the pattern", json!("not allowed")),
        ("punctuation outside the pattern", json!("list!")),
    ];
    for (case, name) in cases {
        let (status, out) = harness
            .call("CreateAddressList", json!({ "AddressListName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // The longest name the bound admits is accepted, so the refusals above are the bound
    // and not a blanket rejection.
    let accepted = harness
        .create(json!({ "AddressListName": "y".repeat(255) }))
        .await;
    assert_well_formed_id("255 characters, at the maximum length", &accepted);
}

/// The address-list store is BOUNDED: a create past the per-account quota is refused with
/// the modelled `ServiceQuotaExceededException`, and the refusal does not corrupt what is
/// already stored.
#[tokio::test]
async fn creates_past_the_address_list_quota_are_refused() {
    let harness = Harness::new();

    let mut created: Vec<String> = Vec::new();
    for index in 0..MAX_ADDRESS_LISTS {
        created.push(
            harness
                .create(json!({ "AddressListName": format!("quota-{index:04}") }))
                .await,
        );
    }

    // Rows: a plain create, and one carrying a ClientToken that was never used — neither
    // may be served past the quota.
    let cases: [(&str, Value); 2] = [
        ("plain", json!({ "AddressListName": "quota-overflow" })),
        (
            "with a fresh ClientToken",
            json!({ "AddressListName": "quota-overflow-2", "ClientToken": "never-seen" }),
        ),
    ];
    for (case, body) in cases {
        let (status, out) = harness.call("CreateAddressList", body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected a create past the quota to answer 402, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code, got {out:?}"
        );
    }

    // Every address list created before the quota was reached is still readable.
    for id in &created {
        let (status, out) = harness
            .call("GetAddressList", json!({ "AddressListId": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {id:?}: a stored address list must survive a refused create, got {status} \
             {out:?}"
        );
    }
}
