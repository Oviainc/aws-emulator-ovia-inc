// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the b2bi trading-partner onboarding slice: Profile,
//! Capability and Partnership create / get / list / delete, driven over the wire
//! (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same router the
//! emulator boots, so every assertion is about what a real SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one
//! recorded request: the rows differ along whatever the rule generalises over
//! (enum value, optional member present or absent, list size, page size, signing
//! region, resource kind).
//!
//! The wire harness below (`b2bi_router`, `call`, `error_code`, `text`,
//! `is_generated_id`) is `pub(crate)` so the other suites in this test target reuse it
//! rather than growing a second copy.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region most requests below are signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so an ARN that ignored the request's SigV4 scope would show up.
pub(crate) const SIGNING_REGION: &str = "us-west-2";

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How many hex digits follow the prefix of a generated b2bi resource id, per the
/// `p-<17 hex>` ids in this service's own Smithy model examples.
pub(crate) const ID_HEX_DIGITS: usize = 17;

/// A router with the b2bi service and the CloudWatch Logs service on one shared
/// engine — what `build_router` assembles, narrowed to the services under test.
///
/// CloudWatch Logs is mounted because `CreateProfile` provisions a logging-ENABLED
/// profile's log group by calling it in process, exactly as the assembled emulator does.
pub(crate) fn b2bi_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(service_com_amazonaws_b2bi_b2bi::EmulatedService::new(Arc::clone(
        &engine,
    )));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// Invoke one operation and return `(status, body)`.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/b2bi/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("x-amz-target", format!("B2BI.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Invoke one operation signed for [`SIGNING_REGION`].
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_signed_for(router, SIGNING_REGION, operation, body).await
}

/// The wire error code (`__type`) of an error body.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or("<no __type>").to_string()
}

/// A string member, or `""` when absent.
pub(crate) fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or_default().to_string()
}

/// Is `id` a `<prefix>-<17 hex>` id that also satisfies the modelled
/// `len 1..64` / `^[a-zA-Z0-9_-]+$` constraint every b2bi resource id carries?
pub(crate) fn is_generated_id(id: &str, prefix: &str) -> bool {
    let modelled = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let shaped = id
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|rest| rest.len() == ID_HEX_DIGITS && rest.chars().all(|c| c.is_ascii_hexdigit()));
    modelled && shaped
}

/// One row of the capability table: the case name, the X12 transaction set and version,
/// the optional `capabilityDirection`, and the optional `instructionsDocuments`.
type CapabilityCase<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, Option<Value>);

/// A valid `CapabilityConfiguration` naming `transformer_id`, for tests that only need
/// capabilities to exist. Member names are the union/structure members of
/// `com.amazonaws.b2bi#CapabilityConfiguration`.
pub(crate) fn capability_configuration(
    transformer_id: &str,
    transaction_set: &str,
    version: &str,
) -> Value {
    json!({
        "edi": {
            "type": { "x12Details": { "transactionSet": transaction_set, "version": version } },
            "inputLocation": { "bucketName": "inbound-bucket", "key": "in/" },
            "outputLocation": { "bucketName": "outbound-bucket", "key": "out/" },
            "transformerId": transformer_id
        }
    })
}

/// Create a transformer and return its system-assigned `transformerId`.
///
/// A capability RESOLVES `configuration.edi.transformerId` in this service's store, so a
/// capability needs a transformer that really exists; this is the call a client makes to
/// get one. `name` is reduced to the characters the modelled `TransformerName`
/// (`^[a-zA-Z0-9_-]{1,512}$`) accepts, so a case label can be handed straight in.
pub(crate) async fn given_transformer(router: &MultiServiceRouter, name: &str) -> String {
    let modelled_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let (status, body) = call(
        router,
        "CreateTransformer",
        json!({
            "name": modelled_name,
            "inputConversion": {
                "fromFormat": "X12",
                "formatOptions": {
                    "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, 201, "CreateTransformer({name}) failed: {body}");
    text(&body, "transformerId")
}

/// Create a profile and return its `profileId`.
pub(crate) async fn given_profile(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateProfile",
        json!({
            "name": name,
            "phone": "555-555-0100",
            "businessName": format!("{name} Corp"),
            "logging": "DISABLED"
        }),
    )
    .await;
    assert_eq!(status, 201, "CreateProfile({name}) failed: {body}");
    text(&body, "profileId")
}

/// Create a capability — and the transformer it names — and return its `capabilityId`.
pub(crate) async fn given_capability(router: &MultiServiceRouter, name: &str) -> String {
    let transformer_id = given_transformer(router, &format!("{name}-transformer")).await;
    let (status, body) = call(
        router,
        "CreateCapability",
        json!({
            "name": name,
            "type": "edi",
            "configuration": capability_configuration(&transformer_id, "X12_850", "VERSION_4010")
        }),
    )
    .await;
    assert_eq!(status, 201, "CreateCapability({name}) failed: {body}");
    text(&body, "capabilityId")
}

/// Create a partnership under `profile_id` and return its `partnershipId`.
pub(crate) async fn given_partnership(
    router: &MultiServiceRouter,
    profile_id: &str,
    capability_id: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePartnership",
        json!({
            "profileId": profile_id,
            "name": name,
            "email": "partner@example.com",
            "capabilities": [capability_id]
        }),
    )
    .await;
    assert_eq!(status, 201, "CreatePartnership({name}) failed: {body}");
    text(&body, "partnershipId")
}

/// RULE: `CreateProfile` mints a profile, persists it, and answers with the stored
/// record; the ARN names the request's signing region and the canonical account; a
/// `logGroupName` exists exactly when `logging` is `ENABLED`, as `b2bi/<profileId>-Logs`;
/// and `GetProfile` / `ListProfiles` report that same record.
///
/// The rows span both values of the modelled `Logging` enum, the optional `email`
/// present and absent, two phone spellings the modelled pattern accepts, and two
/// signing regions.
#[tokio::test]
async fn a_created_profile_is_readable_listable_and_carries_its_derived_members() {
    // (case, region, logging, email, phone)
    let cases: &[(&str, &str, &str, Option<&str>, &str)] = &[
        ("logging off, no email", "us-west-2", "DISABLED", None, "555-555-0100"),
        (
            "logging on, with email",
            "us-west-2",
            "ENABLED",
            Some("ops@example.com"),
            "+1 (555) 555-0199",
        ),
        ("logging on, no email", "eu-central-1", "ENABLED", None, "5550100"),
        (
            "logging off, with email",
            "eu-central-1",
            "DISABLED",
            Some("a.b-c@example.co.uk"),
            "555 555 0100 ext. 7",
        ),
    ];

    let router = b2bi_router();
    let mut minted = HashSet::new();

    for (case, region, logging, email, phone) in cases {
        let mut request = json!({
            "name": format!("profile for {case}"),
            "phone": phone,
            "businessName": format!("business for {case}"),
            "logging": logging
        });
        if let Some(email) = email {
            request["email"] = json!(email);
        }

        let (status, created) = call_signed_for(&router, region, "CreateProfile", request).await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");

        let profile_id = text(&created, "profileId");
        assert!(
            is_generated_id(&profile_id, "p"),
            "case {case:?}: expected a system-generated `p-` profileId, got {profile_id:?}"
        );
        assert!(
            minted.insert(profile_id.clone()),
            "case {case:?}: profileId {profile_id:?} was minted twice"
        );
        assert_eq!(
            text(&created, "profileArn"),
            format!("arn:aws:b2bi:{region}:{ACCOUNT}:profile/{profile_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );
        assert_eq!(
            created["logging"], json!(logging),
            "case {case:?}: logging must be echoed from the stored record"
        );
        assert_eq!(
            created["email"],
            email.map(|e| json!(e)).unwrap_or(Value::Null),
            "case {case:?}: an absent email must stay absent, got {:?}",
            created["email"]
        );

        let expected_log_group = if *logging == "ENABLED" {
            json!(format!("b2bi/{profile_id}-Logs"))
        } else {
            Value::Null
        };
        assert_eq!(
            created["logGroupName"], expected_log_group,
            "case {case:?}: logGroupName must exist exactly when logging is ENABLED, got {:?}",
            created["logGroupName"]
        );
        assert!(
            !created["createdAt"].is_null(),
            "case {case:?}: createdAt must be present, got {:?}",
            created["createdAt"]
        );

        // Round trip: the reader reports the stored record, member for member.
        let (status, fetched) =
            call(&router, "GetProfile", json!({ "profileId": profile_id })).await;
        assert_eq!(status, 200, "case {case:?}: GetProfile failed: {fetched}");
        for member in [
            "profileId",
            "profileArn",
            "name",
            "email",
            "phone",
            "businessName",
            "logging",
            "logGroupName",
            "createdAt",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetProfile disagrees with CreateProfile on {member}: {:?} vs {:?}",
                fetched[member], created[member]
            );
        }
        assert_eq!(
            fetched["modifiedAt"], Value::Null,
            "case {case:?}: a never-updated profile has no modifiedAt, got {:?}",
            fetched["modifiedAt"]
        );
    }

    // Every created profile is enumerable, with the summary members off the same record.
    let (status, listed) = call(&router, "ListProfiles", json!({})).await;
    assert_eq!(status, 200, "ListProfiles failed: {listed}");
    let profiles = listed["profiles"].as_array().expect("profiles is a list");
    assert_eq!(
        profiles.len(),
        cases.len(),
        "ListProfiles must report every created profile, got {listed}"
    );
    let listed_ids: HashSet<String> = profiles
        .iter()
        .map(|summary| text(summary, "profileId"))
        .collect();
    assert_eq!(
        listed_ids, minted,
        "ListProfiles must enumerate exactly the created profiles"
    );
}

/// RULE: `CreateCapability` persists the whole `configuration` union and the optional
/// `instructionsDocuments` verbatim, so `GetCapability` returns them unchanged; an
/// absent optional list reads back absent, not as an empty list.
///
/// The rows span two X12 transaction sets and versions (values from the modelled
/// `X12TransactionSet` / `X12Version` enums), the optional `capabilityDirection`
/// present and absent, and 0, 1 and 2 instruction documents plus the member omitted.
#[tokio::test]
async fn a_created_capability_round_trips_its_configuration_and_instructions_documents() {
    let documents = |count: usize| -> Value {
        json!((0..count)
            .map(|index| json!({ "bucketName": "instructions-bucket", "key": format!("doc-{index}.txt") }))
            .collect::<Vec<_>>())
    };

    // (case, transactionSet, version, capabilityDirection, instructionsDocuments)
    let cases: &[CapabilityCase<'_>] = &[
        ("850 inbound, no documents", "X12_850", "VERSION_4010", Some("INBOUND"), None),
        (
            "810 outbound, empty document list",
            "X12_810",
            "VERSION_5010",
            Some("OUTBOUND"),
            Some(documents(0)),
        ),
        (
            "997 no direction, one document",
            "X12_997",
            "VERSION_4030",
            None,
            Some(documents(1)),
        ),
        (
            "214 outbound, two documents",
            "X12_214",
            "VERSION_4060",
            Some("OUTBOUND"),
            Some(documents(2)),
        ),
    ];

    let router = b2bi_router();
    let transformer_id = given_transformer(&router, "edi-850-to-json").await;
    let mut minted = HashSet::new();

    for (case, transaction_set, version, direction, instructions) in cases {
        let mut configuration = capability_configuration(&transformer_id, transaction_set, version);
        if let Some(direction) = direction {
            configuration["edi"]["capabilityDirection"] = json!(direction);
        }
        let mut request = json!({
            "name": format!("capability for {case}"),
            "type": "edi",
            "configuration": configuration.clone()
        });
        if let Some(instructions) = instructions {
            request["instructionsDocuments"] = instructions.clone();
        }

        let (status, created) = call(&router, "CreateCapability", request).await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");

        let capability_id = text(&created, "capabilityId");
        assert!(
            is_generated_id(&capability_id, "ca"),
            "case {case:?}: expected a system-generated `ca-` capabilityId, got {capability_id:?}"
        );
        assert!(
            minted.insert(capability_id.clone()),
            "case {case:?}: capabilityId {capability_id:?} was minted twice"
        );
        assert_eq!(
            text(&created, "capabilityArn"),
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:capability/{capability_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );
        assert_eq!(
            created["configuration"], configuration,
            "case {case:?}: the stored configuration must come back unchanged, got {:?}",
            created["configuration"]
        );

        let (status, fetched) =
            call(&router, "GetCapability", json!({ "capabilityId": capability_id })).await;
        assert_eq!(status, 200, "case {case:?}: GetCapability failed: {fetched}");
        assert_eq!(
            fetched["configuration"], configuration,
            "case {case:?}: GetCapability must return the persisted configuration, got {:?}",
            fetched["configuration"]
        );
        let expected_instructions = instructions.clone().unwrap_or(Value::Null);
        assert_eq!(
            fetched["instructionsDocuments"], expected_instructions,
            "case {case:?}: instructionsDocuments must round trip exactly, got {:?}",
            fetched["instructionsDocuments"]
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: createdAt must be stable across reads"
        );
        assert_eq!(
            fetched["modifiedAt"], Value::Null,
            "case {case:?}: a never-updated capability has no modifiedAt"
        );
    }

    let (status, listed) = call(&router, "ListCapabilities", json!({})).await;
    assert_eq!(status, 200, "ListCapabilities failed: {listed}");
    let listed_ids: HashSet<String> = listed["capabilities"]
        .as_array()
        .expect("capabilities is a list")
        .iter()
        .map(|summary| text(summary, "capabilityId"))
        .collect();
    assert_eq!(
        listed_ids, minted,
        "ListCapabilities must enumerate exactly the created capabilities"
    );
}

/// RULE: a create that names another resource by id resolves that id in the store, and
/// answers the modelled `ResourceNotFoundException` (HTTP 404) when it is absent —
/// never a success. Rows span several well-formed but unknown profile ids.
#[tokio::test]
async fn creating_a_partnership_under_an_unknown_profile_is_a_404() {
    let router = b2bi_router();
    let capability_id = given_capability(&router, "edi-850").await;

    // Well-formed per the modelled `^[a-zA-Z0-9_-]+$` ProfileId pattern, but not stored.
    let unknown: &[&str] = &[
        "doesnotexist1",
        "p-00000000000000000",
        "P_1",
        "a-very-long-but-unknown-profile-identifier-0123456789",
    ];

    for case in unknown {
        let (status, body) = call(
            &router,
            "CreatePartnership",
            json!({
                "profileId": case,
                "name": "Partner One",
                "email": "partner@example.com",
                "capabilities": [capability_id.clone()]
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown profileId must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // And the same call against a profile that exists succeeds.
    let profile_id = given_profile(&router, "acme").await;
    let (status, body) = call(
        &router,
        "CreatePartnership",
        json!({
            "profileId": profile_id,
            "name": "Partner One",
            "email": "partner@example.com",
            "capabilities": [capability_id]
        }),
    )
    .await;
    assert_eq!(status, 201, "a known profileId must succeed, got {status} {body}");
}

/// RULE: `CreatePartnership` persists `profileId`, `name`, `email`, `phone`,
/// `capabilities` and `capabilityOptions`, mints a `partnershipId` and a
/// `tradingPartnerId`, and `GetPartnership` reports exactly that record.
///
/// Rows span capability-list sizes 1..3, the optional `phone` present and absent, and
/// `capabilityOptions` absent / outbound-only / inbound-and-outbound.
#[tokio::test]
async fn a_created_partnership_round_trips_every_persisted_member() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capabilities: Vec<String> = {
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(given_capability(&router, &format!("edi-{index}")).await);
        }
        ids
    };

    let outbound = json!({
        "outboundEdi": {
            "x12": { "common": { "validateEdi": true, "gs05TimeFormat": "HHMM" } }
        }
    });
    let both = json!({
        "outboundEdi": { "x12": { "wrapOptions": { "wrapBy": "SEGMENT" } } },
        "inboundEdi": {
            "x12": {
                "acknowledgmentOptions": {
                    "functionalAcknowledgment": "GENERATE_ALL_SEGMENTS",
                    "technicalAcknowledgment": "DO_NOT_GENERATE"
                }
            }
        }
    });

    // (case, capability count, phone, capabilityOptions)
    let cases: &[(&str, usize, Option<&str>, Option<&Value>)] = &[
        ("one capability, no phone, no options", 1, None, None),
        ("two capabilities, phone, outbound options", 2, Some("555-555-0133"), Some(&outbound)),
        ("three capabilities, phone, both options", 3, Some("+1 555 555 0144"), Some(&both)),
    ];

    let mut minted_partnerships = HashSet::new();
    let mut minted_trading_partners = HashSet::new();

    for (case, capability_count, phone, options) in cases {
        let bound: Vec<String> = capabilities[..*capability_count].to_vec();
        let mut request = json!({
            "profileId": profile_id.clone(),
            "name": format!("partner for {case}"),
            "email": "partner@example.com",
            "capabilities": bound.clone()
        });
        if let Some(phone) = phone {
            request["phone"] = json!(phone);
        }
        if let Some(options) = options {
            request["capabilityOptions"] = (*options).clone();
        }

        let (status, created) = call(&router, "CreatePartnership", request).await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status} {created}");

        let partnership_id = text(&created, "partnershipId");
        let trading_partner_id = text(&created, "tradingPartnerId");
        assert!(
            is_generated_id(&partnership_id, "ps"),
            "case {case:?}: expected a system-generated `ps-` partnershipId, got {partnership_id:?}"
        );
        assert!(
            is_generated_id(&trading_partner_id, "tp"),
            "case {case:?}: expected a system-generated `tp-` tradingPartnerId, got {trading_partner_id:?}"
        );
        assert!(
            minted_partnerships.insert(partnership_id.clone()),
            "case {case:?}: partnershipId {partnership_id:?} was minted twice"
        );
        assert!(
            minted_trading_partners.insert(trading_partner_id.clone()),
            "case {case:?}: tradingPartnerId {trading_partner_id:?} was minted twice"
        );
        assert_eq!(
            text(&created, "partnershipArn"),
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:partnership/{partnership_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );

        let (status, fetched) = call(
            &router,
            "GetPartnership",
            json!({ "partnershipId": partnership_id }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: GetPartnership failed: {fetched}");
        assert_eq!(
            fetched["profileId"], json!(profile_id),
            "case {case:?}: the partnership must report the profile it was created under"
        );
        assert_eq!(
            fetched["capabilities"], json!(bound),
            "case {case:?}: the capability list must round trip in order, got {:?}",
            fetched["capabilities"]
        );
        assert_eq!(
            fetched["capabilityOptions"],
            options.map(|o| (*o).clone()).unwrap_or(Value::Null),
            "case {case:?}: capabilityOptions must round trip exactly, got {:?}",
            fetched["capabilityOptions"]
        );
        assert_eq!(
            fetched["phone"],
            phone.map(|p| json!(p)).unwrap_or(Value::Null),
            "case {case:?}: an absent phone must stay absent, got {:?}",
            fetched["phone"]
        );
        for member in [
            "partnershipId",
            "partnershipArn",
            "name",
            "email",
            "tradingPartnerId",
            "createdAt",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetPartnership disagrees with CreatePartnership on {member}"
            );
        }
        assert_eq!(
            fetched["modifiedAt"], Value::Null,
            "case {case:?}: a never-updated partnership has no modifiedAt"
        );
    }
}

/// RULE: `maxResults` is a real page size and `nextToken` resumes the enumeration, so
/// walking the pages yields every resource exactly once, in creation order, and the
/// last page carries no token.
///
/// Rows span page sizes 1, 2, 3, 6, 7 and the modelled `MaxResults` maximum (100) over
/// a 7-element collection, plus `maxResults` omitted entirely.
#[tokio::test]
async fn paging_a_list_yields_every_resource_once_in_creation_order() {
    const TOTAL: usize = 7;

    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;

    let mut created_in_order = Vec::new();
    for index in 0..TOTAL {
        created_in_order
            .push(given_partnership(&router, &profile_id, &capability_id, &format!("partner-{index}")).await);
    }

    // `None` means "maxResults omitted"; 1 and 100 are the published bounds of the
    // modelled `MaxResults` range.
    let page_sizes: &[Option<i32>] = &[Some(1), Some(2), Some(3), Some(6), Some(7), Some(100), None];

    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let mut walked = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({ "profileId": profile_id.clone() });
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }

            let (status, body) = call(&router, "ListPartnerships", request).await;
            assert_eq!(status, 200, "case {case:?}: ListPartnerships failed: {body}");
            let page = body["partnerships"].as_array().expect("partnerships is a list");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= *size as usize,
                    "case {case:?}: a page must not exceed maxResults, got {} items",
                    page.len()
                );
            }
            walked.extend(page.iter().map(|summary| text(summary, "partnershipId")));

            pages += 1;
            assert!(
                pages <= TOTAL + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );

            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, created_in_order,
            "case {case:?}: paging must yield every partnership once, in creation order"
        );
    }
}

/// RULE: a `maxResults` outside the modelled `range 1..100` is refused with HTTP 400
/// before any handler runs — one value past each published bound.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = b2bi_router();

    for operation in ["ListProfiles", "ListCapabilities", "ListPartnerships"] {
        for max_results in [0, 101] {
            let case = format!("{operation} maxResults={max_results}");
            let (status, body) =
                call(&router, operation, json!({ "maxResults": max_results })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a maxResults outside 1..100, got {status} {body}"
            );
        }
    }
}

/// RULE: a `nextToken` this service never issued is refused with the modelled
/// `ValidationException` (HTTP 400), on every paginated operation.
#[tokio::test]
async fn an_unissued_next_token_is_refused_as_a_validation_error() {
    let router = b2bi_router();

    for operation in ["ListProfiles", "ListCapabilities", "ListPartnerships"] {
        for token in ["not-a-token", "zz", "abc"] {
            let case = format!("{operation} nextToken={token:?}");
            let (status, body) = call(&router, operation, json!({ "nextToken": token })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: the optional `profileId` filter narrows `ListPartnerships` to the partnerships
/// created under that profile, and nothing else. Rows span two profiles holding
/// different counts, the unfiltered enumeration, and a profile with no partnerships.
#[tokio::test]
async fn list_partnerships_reports_only_the_named_profiles_partnerships() {
    let router = b2bi_router();
    let capability_id = given_capability(&router, "edi-850").await;

    let busy = given_profile(&router, "busy").await;
    let quiet = given_profile(&router, "quiet").await;
    let empty = given_profile(&router, "empty").await;

    let mut expected_busy = Vec::new();
    for index in 0..3 {
        expected_busy.push(given_partnership(&router, &busy, &capability_id, &format!("busy-{index}")).await);
    }
    let mut expected_quiet = Vec::new();
    for index in 0..2 {
        expected_quiet
            .push(given_partnership(&router, &quiet, &capability_id, &format!("quiet-{index}")).await);
    }

    let mut everything = expected_busy.clone();
    everything.extend(expected_quiet.clone());

    // (case, filter, expected ids)
    let cases: &[(&str, Option<&str>, &Vec<String>)] = &[
        ("busy profile", Some(busy.as_str()), &expected_busy),
        ("quiet profile", Some(quiet.as_str()), &expected_quiet),
        ("profile with none", Some(empty.as_str()), &Vec::new()),
        ("unknown profile", Some("p-ffffffffffffffffe"), &Vec::new()),
        ("no filter", None, &everything),
    ];

    for (case, filter, expected) in cases {
        let request = match filter {
            Some(profile_id) => json!({ "profileId": profile_id }),
            None => json!({}),
        };
        let (status, body) = call(&router, "ListPartnerships", request).await;
        assert_eq!(status, 200, "case {case:?}: ListPartnerships failed: {body}");
        let got: Vec<String> = body["partnerships"]
            .as_array()
            .expect("partnerships is a list")
            .iter()
            .map(|summary| text(summary, "partnershipId"))
            .collect();
        assert_eq!(
            &got, *expected,
            "case {case:?}: the profileId filter reported the wrong partnerships"
        );
    }
}

/// RULE: a delete removes the record, so a later get for that id is the modelled
/// `ResourceNotFoundException` (404) and the listing no longer carries it; deleting an
/// id that is not stored is itself a 404. Rows span all three resource kinds.
#[tokio::test]
async fn a_deleted_resource_is_gone_from_both_get_and_list() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;
    let partnership_id =
        given_partnership(&router, &profile_id, &capability_id, "partner-one").await;

    // (case, delete op, get op, id member, id, list op, items member, id member in summary)
    // Deleted in dependency order: the partnership references the profile and the
    // capability, which `ConflictException` protects while it exists.
    let cases: &[(&str, &str, &str, &str, &str, &str, &str)] = &[
        (
            "partnership",
            "DeletePartnership",
            "GetPartnership",
            "partnershipId",
            partnership_id.as_str(),
            "ListPartnerships",
            "partnerships",
        ),
        (
            "capability",
            "DeleteCapability",
            "GetCapability",
            "capabilityId",
            capability_id.as_str(),
            "ListCapabilities",
            "capabilities",
        ),
        (
            "profile",
            "DeleteProfile",
            "GetProfile",
            "profileId",
            profile_id.as_str(),
            "ListProfiles",
            "profiles",
        ),
    ];

    for (case, delete_op, get_op, id_member, id, list_op, items) in cases {
        let (status, body) = call(&router, delete_op, json!({ *id_member: id })).await;
        assert_eq!(status, 200, "case {case:?}: {delete_op} failed: {status} {body}");

        let (status, body) = call(&router, get_op, json!({ *id_member: id })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: {get_op} after the delete must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = call(&router, list_op, json!({})).await;
        assert_eq!(status, 200, "case {case:?}: {list_op} failed: {body}");
        let still_listed = body[*items]
            .as_array()
            .expect("the list member is a list")
            .iter()
            .any(|summary| summary[*id_member] == json!(id));
        assert!(
            !still_listed,
            "case {case:?}: {list_op} still reports the deleted {id}: {body}"
        );

        // Deleting it a second time is a 404: the record really is gone.
        let (status, body) = call(&router, delete_op, json!({ *id_member: id })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a repeated {delete_op} must be 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// RULE: deleting a resource another resource still uses is the modelled
/// `ConflictException` (HTTP 409); once the user is gone the delete succeeds. Rows span
/// the profile a partnership was created under and a capability it lists.
#[tokio::test]
async fn deleting_a_resource_a_partnership_still_uses_is_a_conflict() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;
    let partnership_id =
        given_partnership(&router, &profile_id, &capability_id, "partner-one").await;

    // (case, delete op, id member, id)
    let cases: &[(&str, &str, &str, &str)] = &[
        ("profile in use", "DeleteProfile", "profileId", profile_id.as_str()),
        (
            "capability in use",
            "DeleteCapability",
            "capabilityId",
            capability_id.as_str(),
        ),
    ];

    for (case, delete_op, id_member, id) in cases {
        let (status, body) = call(&router, delete_op, json!({ *id_member: id })).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 while the resource is in use, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }

    let (status, body) = call(
        &router,
        "DeletePartnership",
        json!({ "partnershipId": partnership_id }),
    )
    .await;
    assert_eq!(status, 200, "DeletePartnership failed: {status} {body}");

    for (case, delete_op, id_member, id) in cases {
        let (status, body) = call(&router, delete_op, json!({ *id_member: id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once unused, {delete_op} must succeed, got {status} {body}"
        );
    }
}

/// RULE: a create repeated with the same `@idempotencyToken` answers with the resource
/// the first call created, rather than minting a second one; a different token mints a
/// new resource. Rows span all three creates.
#[tokio::test]
async fn repeating_a_create_with_the_same_client_token_returns_the_same_resource() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;
    let transformer_id = given_transformer(&router, "idempotent-transformer").await;

    let profile_request = |token: &str| {
        json!({
            "name": "idempotent profile",
            "phone": "555-555-0155",
            "businessName": "Idempotent Corp",
            "logging": "ENABLED",
            "clientToken": token
        })
    };
    let capability_request = move |token: &str| {
        json!({
            "name": "idempotent capability",
            "type": "edi",
            "configuration": capability_configuration(&transformer_id, "X12_850", "VERSION_4010"),
            "clientToken": token
        })
    };
    let partnership_request = |token: &str| {
        json!({
            "profileId": profile_id.clone(),
            "name": "idempotent partner",
            "email": "partner@example.com",
            "capabilities": [capability_id.clone()],
            "clientToken": token
        })
    };

    // (case, operation, id member, request builder, the list op and its items member)
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, &str, Box<dyn Fn(&str) -> Value>, &str, &str)> = vec![
        (
            "profile",
            "CreateProfile",
            "profileId",
            Box::new(profile_request),
            "ListProfiles",
            "profiles",
        ),
        (
            "capability",
            "CreateCapability",
            "capabilityId",
            Box::new(capability_request),
            "ListCapabilities",
            "capabilities",
        ),
        (
            "partnership",
            "CreatePartnership",
            "partnershipId",
            Box::new(partnership_request),
            "ListPartnerships",
            "partnerships",
        ),
    ];

    for (case, operation, id_member, request, list_op, items) in &cases {
        let (_, before) = call(&router, list_op, json!({})).await;
        let count_before = before[*items].as_array().map(Vec::len).unwrap_or_default();

        let (status, first) = call(&router, operation, request("token-alpha")).await;
        assert_eq!(status, 201, "case {case:?}: first create failed: {status} {first}");
        let (status, replay) = call(&router, operation, request("token-alpha")).await;
        assert_eq!(status, 201, "case {case:?}: replay failed: {status} {replay}");
        assert_eq!(
            replay[*id_member], first[*id_member],
            "case {case:?}: the replay must answer with the first resource, got {:?} vs {:?}",
            replay[*id_member], first[*id_member]
        );
        assert_eq!(
            replay["createdAt"], first["createdAt"],
            "case {case:?}: the replay must report the stored createdAt"
        );

        let (status, other) = call(&router, operation, request("token-beta")).await;
        assert_eq!(status, 201, "case {case:?}: second token failed: {status} {other}");
        assert_ne!(
            other[*id_member], first[*id_member],
            "case {case:?}: a different clientToken must mint a different resource"
        );

        let (_, after) = call(&router, list_op, json!({})).await;
        let count_after = after[*items].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            count_after,
            count_before + 2,
            "case {case:?}: two distinct tokens must add exactly two resources"
        );
    }
}

/// RULE: a create refuses once its store is full, with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) naming the limit that was reached — AWS's
/// documented five-profile limit for profiles, and the emulator's own residency bound
/// (`MAX_RECORDS_PER_STORE`) for the stores AWS publishes no limit for, so repeated
/// unauthenticated creates cannot grow the process's memory without limit. Rows span all
/// three stores, each on its own engine.
#[tokio::test]
async fn a_create_is_refused_once_its_store_is_full() {
    use service_com_amazonaws_b2bi_b2bi::state::{
        MAX_PROFILES_PER_ACCOUNT, MAX_RECORDS_PER_STORE,
    };

    // (case, resource type, how many fit)
    for (case, bound) in [
        ("profile", MAX_PROFILES_PER_ACCOUNT),
        ("capability", MAX_RECORDS_PER_STORE),
        ("partnership", MAX_RECORDS_PER_STORE),
    ] {
        let router = b2bi_router();

        // The partnership store needs one profile and one capability to point at, and the
        // capability store needs one transformer; they live in their own stores, so they
        // do not consume slots of the store being filled.
        let (profile_id, capability_id) = if case == "partnership" {
            (
                given_profile(&router, "acme").await,
                given_capability(&router, "edi-850").await,
            )
        } else {
            (String::new(), String::new())
        };
        let transformer_id = if case == "capability" {
            given_transformer(&router, "bound-transformer").await
        } else {
            String::new()
        };

        let request = |index: usize| match case {
            "profile" => (
                "CreateProfile",
                json!({
                    "name": format!("profile-{index}"),
                    "phone": "555-555-0100",
                    "businessName": "Acme Corp",
                    "logging": "DISABLED"
                }),
            ),
            "capability" => (
                "CreateCapability",
                json!({
                    "name": format!("capability-{index}"),
                    "type": "edi",
                    "configuration": capability_configuration(
                        &transformer_id,
                        "X12_850",
                        "VERSION_4010"
                    )
                }),
            ),
            _ => (
                "CreatePartnership",
                json!({
                    "profileId": profile_id.clone(),
                    "name": format!("partner-{index}"),
                    "email": "partner@example.com",
                    "capabilities": [capability_id.clone()]
                }),
            ),
        };

        // Fill the store exactly to the bound. The profile and capability the
        // partnership case needs live in their own stores, so they consume no slot of
        // the store being filled.
        for index in 0..bound {
            let (operation, body) = request(index);
            let (status, response) = call(&router, operation, body).await;
            assert_eq!(
                status, 201,
                "case {case:?}: create #{index} below the bound must succeed, got {status} {response}"
            );
        }

        let (operation, body) = request(bound);
        let (status, response) = call(&router, operation, body).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a create past the {bound}-record bound must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {response}"
        );
        assert!(
            text(&response, "message").contains(&bound.to_string()),
            "case {case:?}: the refusal must name the limit it reached, got {:?}",
            text(&response, "message")
        );
        assert!(
            response["quotaCode"].as_str().is_some(),
            "case {case:?}: the refusal must name the quota that was exceeded, got {response}"
        );
    }
}

/// RULE: the idempotency map is bounded as well — it evicts the oldest remembered
/// `clientToken` once `MAX_IDEMPOTENCY_ENTRIES` are held, and a delete releases the
/// entries that pointed at the deleted resource. Asserted directly against the state
/// struct (the ceiling is far above the residency bound, so no sequence of creates can
/// reach it through the wire) and end to end for the release path.
#[tokio::test]
async fn the_idempotency_map_is_bounded_and_released() {
    use service_com_amazonaws_b2bi_b2bi::state::{
        ComAmazonawsB2biB2biState, MAX_IDEMPOTENCY_ENTRIES,
    };

    // The ceiling: recording more tokens than the bound evicts the oldest first.
    let mut state = ComAmazonawsB2biB2biState::default();
    let overshoot = 16;
    for index in 0..MAX_IDEMPOTENCY_ENTRIES + overshoot {
        state.record_idempotency(format!("profile#token-{index}"), &format!("p-{index}"));
    }
    assert_eq!(
        state.idempotency.len(),
        MAX_IDEMPOTENCY_ENTRIES,
        "the idempotency map must stay at its {MAX_IDEMPOTENCY_ENTRIES}-entry ceiling"
    );
    assert_eq!(
        state.idempotency_order.len(),
        MAX_IDEMPOTENCY_ENTRIES,
        "the eviction order must stay in step with the map"
    );
    assert!(
        !state.idempotency.contains_key("profile#token-0"),
        "the oldest remembered token must be the one evicted"
    );
    assert!(
        state
            .idempotency
            .contains_key(&format!("profile#token-{}", MAX_IDEMPOTENCY_ENTRIES + overshoot - 1)),
        "the newest remembered token must be kept"
    );

    // Recording the same key twice is one entry, not two.
    let mut state = ComAmazonawsB2biB2biState::default();
    state.record_idempotency("profile#same".to_string(), "p-1");
    state.record_idempotency("profile#same".to_string(), "p-1");
    assert_eq!(
        (state.idempotency.len(), state.idempotency_order.len()),
        (1, 1),
        "a repeated key must not be remembered twice"
    );

    // The release path: a token dropped on delete, so the next replay mints afresh.
    let router = b2bi_router();
    let profile_id = given_profile(&router, "acme").await;
    let capability_id = given_capability(&router, "edi-850").await;
    let partnership = |token: &str| {
        json!({
            "profileId": profile_id.clone(),
            "name": "partner-one",
            "email": "partner@example.com",
            "capabilities": [capability_id.clone()],
            "clientToken": token
        })
    };

    let (status, first) = call(&router, "CreatePartnership", partnership("token-gamma")).await;
    assert_eq!(status, 201, "CreatePartnership failed: {status} {first}");
    let first_id = text(&first, "partnershipId");

    let (status, body) = call(
        &router,
        "DeletePartnership",
        json!({ "partnershipId": first_id.clone() }),
    )
    .await;
    assert_eq!(status, 200, "DeletePartnership failed: {status} {body}");

    let (status, replay) = call(&router, "CreatePartnership", partnership("token-gamma")).await;
    assert_eq!(status, 201, "the replay after a delete failed: {status} {replay}");
    assert_ne!(
        text(&replay, "partnershipId"),
        first_id,
        "once the resource is deleted its token is released, so the replay mints a new one"
    );
}

/// The operations this slice deliberately leaves unimplemented still answer HTTP 501
/// `NotImplemented` — never an empty success — so the stub census stays honest.
#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    let router = b2bi_router();

    // (case, operation, a request that satisfies the modelled input constraints)
    let cases: &[(&str, &str, Value)] = &[
        ("list transformers", "ListTransformers", json!({})),
        (
            "delete transformer",
            "DeleteTransformer",
            json!({ "transformerId": "t-0123456789abcdef0" }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501 from the scaffold, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
    }
}
