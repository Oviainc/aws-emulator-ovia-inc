// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `ApiMapping` lifecycle:
//! `CreateApiMapping` and `GetApiMapping`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules asserted here, each over a TABLE rather than over one request:
//!
//! * An api mapping is a persisted child of the custom domain name its path names: what
//!   `CreateApiMapping` reports is what a later `GetApiMapping` of the minted
//!   `ApiMappingId` reports.
//! * An operation whose path names a custom domain name the account does not hold is
//!   refused with the modelled `NotFoundException` (404) — the recorded case that filed
//!   the issue, one row of its table — and so is one naming an api that does not exist.
//! * `Stage` must name a stage of the mapped api; a value that names none of them is
//!   incomplete-or-incorrect input, refused with the modelled `BadRequestException`
//!   (400).
//! * One base path of one domain name resolves to exactly one api stage, so a second
//!   mapping for a base path the domain name already maps is refused with the modelled
//!   `ConflictException` (409), and the mapping already stored survives.
//! * An `ApiMappingKey` is a base path: written without a leading separator and with
//!   exactly one separator between segments, each segment drawn from letters, digits and
//!   the punctuation the service documents. Anything else is refused with
//!   `BadRequestException` (400).
//! * Caller-driven growth is bounded: a domain name takes no more mappings than the
//!   store's bound, and the refusal is the modelled `TooManyRequestsException` (429).
//! * `GetApiMappings` reports the mappings the named domain name OWNS, each exactly as
//!   the single-resource reader reports it and in `ApiMappingId` order, in a page of at
//!   most `MaxResults` items chained through `NextToken`; it changes nothing.
//!
//! The rows vary what each rule generalises over: the form of the mapping key (absent,
//! empty, one segment, several segments, every documented punctuation character), the
//! stage it points at, the form of an absent domain name or api, which resource of the
//! request is missing, and the mapping count.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    api_mapping_not_found_message, API_MAPPING_KEY_ILLEGAL_CHARACTER,
    API_MAPPING_KEY_MALFORMED_PATH, API_MAPPING_KEY_PUNCTUATION, INVALID_DOMAIN_NAME_IDENTIFIER,
    INVALID_NEXT_TOKEN_MESSAGE, INVALID_STAGE_IDENTIFIER, MAX_API_MAPPINGS_PER_DOMAIN,
};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call_in_region, carries_nothing, emulator, error_code,
};

/// The region every request in this suite is signed for. Deliberately NOT the
/// emulator's `us-east-1` default, so nothing region-derived can be hardcoded.
pub(crate) const REGION: &str = "ap-southeast-2";

/// The custom domain name every test seeds unless the row is about an absent one.
pub(crate) const DOMAIN: &str = "mapped.example.com";

/// The stage every test seeds on its api. `$default` is the stage a base path mapping
/// most often points at, and its `$` proves the stage reference is compared verbatim.
pub(crate) const STAGE: &str = "$default";

/// A second seeded stage, so a row can move the mapping's target without inventing one.
pub(crate) const OTHER_STAGE: &str = "prod";

/// Send one request signed for [`REGION`] and answer `(status, body)`.
pub(crate) async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

pub(crate) async fn create_api_mapping(
    emulator: &MultiServiceRouter,
    domain: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/apimappings"),
        Some(request.clone()),
    )
    .await
}

async fn get_api_mappings(
    emulator: &MultiServiceRouter,
    domain: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/apimappings{query}"),
        None,
    )
    .await
}

pub(crate) async fn get_api_mapping(
    emulator: &MultiServiceRouter,
    domain: &str,
    api_mapping_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/apimappings/{api_mapping_id}"),
        None,
    )
    .await
}

/// Create one custom domain name, asserting the 201 the model binds to
/// `CreateDomainName`.
pub(crate) async fn seed_domain_name(emulator: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            // AWS Certificate Manager is the only source `CertificateArn` documents.
            "domainNameConfigurations": [{
                "certificateArn": engine::emulator_arn(
                    "acm",
                    REGION,
                    &format!("certificate/{domain}"),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding domain name {domain:?}: expected 201 from CreateDomainName, got {status} {body}"
    );
}

/// Create one HTTP api with the stages `stage_names` names, and answer its `ApiId`.
pub(crate) async fn seed_api(
    emulator: &MultiServiceRouter,
    name: &str,
    stage_names: &[&str],
) -> String {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding api {name:?}: expected 201 from CreateApi, got {status} {body}"
    );
    let api_id = as_str(&body, "apiId");
    for stage_name in stage_names {
        // `CreateApi` quick-creates a `$default` stage only when the request names a
        // target, so every stage this suite maps to is created explicitly — unless the
        // api already holds it.
        let (status, stage) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name })),
        )
        .await;
        assert!(
            status == 201 || status == 409,
            "seeding stage {stage_name:?} of api {name:?}: expected 201, got {status} {stage}"
        );
    }
    api_id
}

/// A fresh emulator holding one custom domain name ([`DOMAIN`]) and one api with the
/// stages [`STAGE`] and [`OTHER_STAGE`]. Answers `(emulator, api_id)`.
pub(crate) async fn seeded() -> (MultiServiceRouter, String) {
    let emulator = emulator();
    seed_domain_name(&emulator, DOMAIN).await;
    let api_id = seed_api(&emulator, "mapped-api", &[STAGE, OTHER_STAGE]).await;
    (emulator, api_id)
}

/// One row of the create/read table.
struct MappingCase {
    label: &'static str,
    /// The `apiMappingKey` the request carries, or `None` when it names none.
    key: Option<&'static str>,
    stage: &'static str,
}

/// The rows span the forms a base path takes: none at all (the root of the domain), the
/// empty path written out, one segment, several segments, digits, and the documented
/// punctuation. The stage varies too, so neither member is read off the other.
fn mapping_cases() -> Vec<MappingCase> {
    vec![
        MappingCase {
            label: "no key at all: the root of the domain",
            key: None,
            stage: STAGE,
        },
        MappingCase {
            label: "one segment",
            key: Some("orders"),
            stage: STAGE,
        },
        MappingCase {
            label: "several segments",
            key: Some("orders/v1/items"),
            stage: OTHER_STAGE,
        },
        MappingCase {
            label: "digits and a dot",
            key: Some("v1.2"),
            stage: STAGE,
        },
        MappingCase {
            // Every character of the documented punctuation class at once; the class
            // itself comes from the crate constant, not from a copy written here.
            label: "every documented punctuation character",
            key: Some("a$-_.+!*'(),z"),
            stage: OTHER_STAGE,
        },
    ]
}

#[tokio::test]
async fn create_api_mapping_persists_the_record_get_api_mapping_reports_back() {
    let (emulator, api_id) = seeded().await;
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in mapping_cases() {
        let label = case.label;
        let mut request = json!({ "apiId": api_id, "stage": case.stage });
        if let Some(key) = case.key {
            request["apiMappingKey"] = json!(key);
        }

        let (status, created) = create_api_mapping(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from CreateApiMapping, got {status} {created}"
        );
        assert_eq!(
            as_str(&created, "apiId"),
            api_id,
            "case {label:?}: expected apiId {api_id:?}, got {created}"
        );
        assert_eq!(
            as_str(&created, "stage"),
            case.stage,
            "case {label:?}: expected stage {:?}, got {created}",
            case.stage
        );
        match case.key {
            // The key the request wrote is reported back byte-identically; a request
            // that named none has no base path to report.
            Some(key) => assert_eq!(
                as_str(&created, "apiMappingKey"),
                key,
                "case {label:?}: expected apiMappingKey {key:?}, got {created}"
            ),
            None => assert!(
                carries_nothing(&created, "apiMappingKey"),
                "case {label:?}: expected no apiMappingKey, got {created}"
            ),
        }

        let api_mapping_id = as_str(&created, "apiMappingId");
        assert_minted_id(label, "apiMappingId", &api_mapping_id);
        assert!(
            minted.insert(api_mapping_id.clone()),
            "case {label:?}: expected a fresh apiMappingId, got the already-minted \
             {api_mapping_id:?}"
        );

        // The round trip: the stored record is what a later reader reports.
        let (status, read) = get_api_mapping(&emulator, DOMAIN, &api_mapping_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetApiMapping, got {status} {read}"
        );
        assert_eq!(
            read, created,
            "case {label:?}: expected GetApiMapping to report the created record, got {read}"
        );
    }

    // The empty key written out is the SAME base path as no key at all (the root of the
    // domain), so it goes on a domain name of its own — and what the request wrote is
    // what the record reports back.
    let label = "the empty key written out";
    seed_domain_name(&emulator, "root.example.com").await;
    let (status, created) = create_api_mapping(
        &emulator,
        "root.example.com",
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200 from CreateApiMapping, got {status} {created}"
    );
    assert_eq!(
        as_str(&created, "apiMappingKey"),
        "",
        "case {label:?}: expected an empty apiMappingKey, got {created}"
    );
    let (status, read) =
        get_api_mapping(&emulator, "root.example.com", &as_str(&created, "apiMappingId")).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200 from GetApiMapping, got {status} {read}"
    );
    assert_eq!(
        read, created,
        "case {label:?}: expected GetApiMapping to report the created record, got {read}"
    );
}

#[tokio::test]
async fn a_domain_name_or_an_api_the_account_does_not_hold_is_not_found() {
    let (emulator, api_id) = seeded().await;

    // The rows vary the form of the absent identifier: a plain name, a sibling of the
    // held one, one differing only in case, a deep label, and an api id that is not
    // minted. The recorded unit that filed the issue is the first domain row.
    let domains = [
        ("a plain absent name", "absent.example.com"),
        ("a sibling of the held name", "other.example.com"),
        ("the held name in a different case", "Mapped.Example.Com"),
        ("a deeper label under the held name", "a.b.mapped.example.com"),
    ];
    for (label, domain) in domains {
        let (status, body) = create_api_mapping(
            &emulator,
            domain,
            &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "orders" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from CreateApiMapping on {domain:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_DOMAIN_NAME_IDENTIFIER,
            "case {label:?}: expected the domain-name refusal message, got {body}"
        );
    }

    let apis = [
        ("an id that was never minted", "nosuchapi0"),
        ("the empty id", ""),
        ("the held id with one character changed", "0000000000"),
    ];
    for (label, absent_api) in apis {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({ "apiId": absent_api, "stage": STAGE, "apiMappingKey": "orders" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from CreateApiMapping for api {absent_api:?}, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
    }

    // Nothing above was persisted: the base path every refused row asked for is still
    // free, so the first create that names it succeeds rather than conflicting.
    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "orders" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the refused calls to have persisted nothing, got {status} {created}"
    );
}

#[tokio::test]
async fn a_stage_that_names_no_stage_of_the_api_is_refused() {
    let (emulator, api_id) = seeded().await;
    // An api of its own, so a stage name that exists on ANOTHER api is still no stage
    // reference for this one.
    let neighbour = seed_api(&emulator, "neighbour-api", &["staging"]).await;

    let rows = [
        ("the empty stage", ""),
        ("a stage name nothing created", "nosuchstage"),
        ("a stage of another api", "staging"),
        ("the held stage with different case", "PROD"),
        ("the default stage spelled without its sigil", "default"),
    ];
    for (label, stage) in rows {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({ "apiId": api_id, "stage": stage, "apiMappingKey": "orders" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from CreateApiMapping with stage {stage:?}, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_STAGE_IDENTIFIER,
            "case {label:?}: expected the stage-reference refusal message, got {body}"
        );
    }

    // The positive control for the same table: the neighbour's own stage IS a reference
    // for the neighbour.
    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &json!({ "apiId": neighbour, "stage": "staging", "apiMappingKey": "orders" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the neighbour's own stage to be a usable reference, got {status} {created}"
    );
}

#[tokio::test]
async fn one_base_path_of_one_domain_name_maps_to_exactly_one_api_stage() {
    let (emulator, api_id) = seeded().await;
    let other_api = seed_api(&emulator, "second-api", &[STAGE]).await;

    // The rows vary the form of the base path whose uniqueness is asserted, including
    // the two spellings of the root mapping, which are the SAME base path.
    let rows = [
        ("no key at all", None),
        ("one segment", Some("orders")),
        ("several segments", Some("orders/v1")),
    ];
    for (label, key) in rows {
        let mut first = json!({ "apiId": api_id, "stage": STAGE });
        if let Some(key) = key {
            first["apiMappingKey"] = json!(key);
        }
        let (status, winner) = create_api_mapping(&emulator, DOMAIN, &first).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from the first CreateApiMapping, got {status} {winner}"
        );
        let winner_id = as_str(&winner, "apiMappingId");

        // What the second request differs in must not matter: a different stage, a
        // different api, the same request, and — for the root — the other spelling of
        // the same base path.
        let mut seconds = vec![
            ("the same request again", first.clone()),
            ("a different stage", {
                let mut request = first.clone();
                request["stage"] = json!(OTHER_STAGE);
                request
            }),
            ("a different api", {
                let mut request = first.clone();
                request["apiId"] = json!(other_api);
                request
            }),
        ];
        if key.is_none() {
            let mut request = first.clone();
            request["apiMappingKey"] = json!("");
            seconds.push(("the empty key, the same root base path", request));
        }

        for (second_label, request) in seconds {
            let (status, body) = create_api_mapping(&emulator, DOMAIN, &request).await;
            assert_eq!(
                status, 409,
                "case {label:?}/{second_label:?}: expected 409 from the second \
                 CreateApiMapping, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {label:?}/{second_label:?}: expected ConflictException, got {body}"
            );
        }

        // The mapping already stored survives every refusal.
        let (status, read) = get_api_mapping(&emulator, DOMAIN, &winner_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the stored mapping to survive, got {status} {read}"
        );
        assert_eq!(
            read, winner,
            "case {label:?}: expected the stored mapping to be untouched, got {read}"
        );
    }

    // The base path identifies the mapping within ITS domain name, so a second domain
    // name may map every one of the same base paths.
    seed_domain_name(&emulator, "second.example.com").await;
    for (label, key) in rows {
        let mut request = json!({ "apiId": api_id, "stage": STAGE });
        if let Some(key) = key {
            request["apiMappingKey"] = json!(key);
        }
        let (status, body) = create_api_mapping(&emulator, "second.example.com", &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the same base path to be free on another domain \
             name, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_api_mapping_key_that_is_not_a_base_path_is_refused() {
    let (emulator, api_id) = seeded().await;

    // A base path is written without a leading separator and with exactly one separator
    // between segments. The rows vary where the offending separator sits.
    let malformed = [
        ("a leading separator", "/orders"),
        ("only a separator", "/"),
        ("consecutive separators", "orders//v1"),
        ("consecutive separators at the end", "orders//"),
        ("both faults at once", "//orders"),
    ];
    for (label, key) in malformed {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": key }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for apiMappingKey {key:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            API_MAPPING_KEY_MALFORMED_PATH,
            "case {label:?}: expected the path-shape refusal message, got {body}"
        );
    }

    // Each segment is drawn from letters, digits and the documented punctuation. The
    // rows span the kinds of character outside that class: non-ASCII letters, a space,
    // and URL punctuation the class does not list.
    let illegal = [
        ("a non-ASCII letter", "caf\u{e9}"),
        ("a non-Latin script", "\u{3053}\u{3093}"),
        ("a space", "my orders"),
        ("a query separator", "orders?page=1"),
        ("a fragment separator", "orders#top"),
        ("an ampersand", "orders&items"),
        ("a colon", "orders:v1"),
        ("a percent escape", "orders%2Fv1"),
        ("a bracket", "orders[0]"),
        ("a backslash", "orders\\v1"),
        ("a tilde", "orders~v1"),
        ("an at sign", "orders@v1"),
        ("an equals sign", "orders=v1"),
        ("a semicolon", "orders;v1"),
    ];
    for (label, key) in illegal {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": key }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for apiMappingKey {key:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            API_MAPPING_KEY_ILLEGAL_CHARACTER,
            "case {label:?}: expected the character-class refusal message, got {body}"
        );
    }

    // The positive control walks the documented punctuation class itself — the crate
    // constant, so a character added to it is exercised here without this table being
    // rewritten — plus one letter and one digit row.
    let mut accepted: Vec<String> = API_MAPPING_KEY_PUNCTUATION
        .chars()
        .map(|character| format!("seg{character}"))
        .collect();
    accepted.push("letters".to_string());
    accepted.push("0123456789".to_string());
    accepted.push("seg/seg2".to_string());
    // Neither refusal speaks of a TRAILING separator, so a key that ends in one is a
    // base path this service accepts.
    accepted.push("trailing/".to_string());
    for key in accepted {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": key }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected a key of the documented class to be accepted, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn caller_driven_mapping_growth_is_bounded() {
    let (emulator, api_id) = seeded().await;

    for index in 0..MAX_API_MAPPINGS_PER_DOMAIN {
        let (status, body) = create_api_mapping(
            &emulator,
            DOMAIN,
            &json!({
                "apiId": api_id,
                "stage": STAGE,
                "apiMappingKey": format!("path{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "mapping {index}: expected 200 from CreateApiMapping, got {status} {body}"
        );
    }

    let (status, body) = create_api_mapping(
        &emulator,
        DOMAIN,
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "onetoomany" }),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 once the domain name holds {MAX_API_MAPPINGS_PER_DOMAIN} mappings, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // The bound is per domain name, so another one still takes a mapping.
    seed_domain_name(&emulator, "spare.example.com").await;
    let (status, body) = create_api_mapping(
        &emulator,
        "spare.example.com",
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "onetoomany" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the bound to be per domain name, got {status} {body}"
    );
}

#[tokio::test]
async fn an_api_mapping_id_the_domain_name_does_not_hold_is_not_found() {
    let (emulator, api_id) = seeded().await;
    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "orders" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from CreateApiMapping, got {status} {created}"
    );
    let held = as_str(&created, "apiMappingId");

    // A mapping is addressed only under the domain name that owns it, so the id of a
    // mapping of ANOTHER domain name is not found here either.
    seed_domain_name(&emulator, "elsewhere.example.com").await;
    let (status, elsewhere) = create_api_mapping(
        &emulator,
        "elsewhere.example.com",
        &json!({ "apiId": api_id, "stage": STAGE, "apiMappingKey": "sibling" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from the neighbouring CreateApiMapping, got {status} {elsewhere}"
    );

    let rows = [
        ("an id nothing minted", "nosuchmap0".to_string()),
        ("the held id uppercased", held.to_uppercase()),
        ("the held id with one character appended", format!("{held}x")),
    ];
    for (label, api_mapping_id) in rows {
        let (status, body) = get_api_mapping(&emulator, DOMAIN, &api_mapping_id).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from GetApiMapping for {api_mapping_id:?}, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            api_mapping_not_found_message(&api_mapping_id),
            "case {label:?}: expected the api-mapping refusal message, got {body}"
        );
    }

    // The held mapping is still readable, and reading it under a domain name the
    // account does not hold is a `NotFoundException` about the domain name.
    let (status, read) = get_api_mapping(&emulator, DOMAIN, &held).await;
    assert_eq!(
        status, 200,
        "expected the held mapping to still be readable, got {status} {read}"
    );
    let (status, body) = get_api_mapping(&emulator, "absent.example.com", &held).await;
    assert_eq!(
        status, 404,
        "expected 404 from GetApiMapping under an absent domain name, got {status} {body}"
    );
    assert_eq!(
        as_str(&body, "message"),
        INVALID_DOMAIN_NAME_IDENTIFIER,
        "expected the domain-name refusal message, got {body}"
    );
}

/// Walk `GetApiMappings` from the first page to the last and answer every
/// `ApiMappingId` seen, in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    domain: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is
    // caught instead of looping forever.
    for _ in 0..total + 2 {
        let query = match &token {
            Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            None => format!("?maxResults={page_size}"),
        };
        let (status, body) = get_api_mappings(emulator, domain, &query).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetApiMappings, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(items.iter().map(|item| as_str(item, "apiMappingId")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn get_api_mappings_pages_every_mapping_exactly_once_at_every_page_size() {
    // The rule generalises over how many mappings the domain name holds and over page
    // size, so the table walks both: each count is paged at every size from 1 up to one
    // past it.
    for total in [0_usize, 1, 2, 5, 11] {
        let (emulator, api_id) = seeded().await;
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            let (status, created) = create_api_mapping(
                &emulator,
                DOMAIN,
                &json!({
                    "apiId": api_id,
                    "stage": if index % 2 == 0 { STAGE } else { OTHER_STAGE },
                    "apiMappingKey": format!("path{index}"),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case total={total}, mapping #{index}: expected 200, got {status} {created}"
            );
            expected.insert(as_str(&created, "apiMappingId"));
        }

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, DOMAIN, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} mappings across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored mappings, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                seen, sorted,
                "case {label:?}: expected the chain in ApiMappingId order, got {seen:?}"
            );
        }

        // Unpaged: every mapping in one answer, with nothing left to report. A domain
        // name holding none still reports an empty collection rather than no collection.
        let (status, body) = get_api_mappings(&emulator, DOMAIN, "").await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from an unpaged GetApiMappings, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(total),
            "case total={total}: an unpaged GetApiMappings must report every mapping, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged GetApiMappings has nothing left to report, got {body}"
        );

        // A listing is a report: repeating it changes nothing a later caller sees.
        let (_, again) = get_api_mappings(&emulator, DOMAIN, "").await;
        assert_eq!(
            again, body,
            "case total={total}: a report must not change what a later caller sees, got {again}"
        );
    }
}

#[tokio::test]
async fn get_api_mappings_reports_the_records_the_single_reader_reports() {
    let (emulator, api_id) = seeded().await;
    seed_domain_name(&emulator, "neighbour.example.com").await;

    // The rows vary the form of the mapping key and the stage, and the second domain
    // name proves a mapping belongs to exactly one domain name's listing.
    let rows = [
        (DOMAIN, None, STAGE),
        (DOMAIN, Some("orders"), OTHER_STAGE),
        (DOMAIN, Some("orders/v1"), STAGE),
        ("neighbour.example.com", Some("orders"), STAGE),
    ];
    let mut expected: Vec<(String, BTreeSet<String>)> = vec![
        (DOMAIN.to_string(), BTreeSet::new()),
        ("neighbour.example.com".to_string(), BTreeSet::new()),
    ];
    for (domain, key, stage) in rows {
        let mut request = json!({ "apiId": api_id, "stage": stage });
        if let Some(key) = key {
            request["apiMappingKey"] = json!(key);
        }
        let (status, created) = create_api_mapping(&emulator, domain, &request).await;
        assert_eq!(
            status, 200,
            "case {domain:?}/{key:?}: expected 200 from CreateApiMapping, got {status} {created}"
        );
        let api_mapping_id = as_str(&created, "apiMappingId");
        for (name, ids) in expected.iter_mut() {
            if name == domain {
                ids.insert(api_mapping_id.clone());
            }
        }

        // The listed record is the one the single-resource reader reports, member for
        // member: the listing projects the store, not a shape of its own.
        let (status, body) = get_api_mappings(&emulator, domain, "").await;
        assert_eq!(
            status, 200,
            "case {domain:?}/{key:?}: expected 200 from GetApiMappings, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        let listed = items
            .iter()
            .find(|item| as_str(item, "apiMappingId") == api_mapping_id)
            .unwrap_or_else(|| {
                panic!("case {domain:?}/{key:?}: expected {api_mapping_id} listed, got {body}")
            });
        let (_, fetched) = get_api_mapping(&emulator, domain, &api_mapping_id).await;
        assert_eq!(
            listed, &fetched,
            "case {domain:?}/{key:?}: expected GetApiMappings to report the record \
             GetApiMapping reports, got {listed} and {fetched}"
        );
    }

    for (domain, ids) in &expected {
        let (status, body) = get_api_mappings(&emulator, domain, "").await;
        assert_eq!(
            status, 200,
            "case {domain:?}: expected 200 from GetApiMappings, got {status} {body}"
        );
        let listed: BTreeSet<String> = body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| as_str(item, "apiMappingId"))
            .collect();
        assert_eq!(
            &listed, ids,
            "case {domain:?}: expected exactly the domain name's own mappings, got {body}"
        );
    }
}

#[tokio::test]
async fn get_api_mappings_refuses_an_absent_domain_name_a_bad_max_results_and_a_foreign_token() {
    let (emulator, api_id) = seeded().await;
    seed_domain_name(&emulator, "neighbour.example.com").await;
    for index in 0..3 {
        for domain in [DOMAIN, "neighbour.example.com"] {
            let (status, created) = create_api_mapping(
                &emulator,
                domain,
                &json!({
                    "apiId": api_id,
                    "stage": STAGE,
                    "apiMappingKey": format!("path{index}"),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "seeding mapping #{index} of {domain:?}: expected 200, got {status} {created}"
            );
        }
    }

    for (label, domain) in [
        ("a plain absent name", "absent.example.com"),
        ("the held name in a different case", "Mapped.Example.Com"),
    ] {
        let (status, body) = get_api_mappings(&emulator, domain, "").await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from GetApiMappings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_DOMAIN_NAME_IDENTIFIER,
            "case {label:?}: expected the domain-name refusal message, got {body}"
        );
    }

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5"] {
        let (status, body) =
            get_api_mappings(&emulator, DOMAIN, &format!("?maxResults={raw}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from GetApiMappings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // A token this reader issued for ONE domain name names no position in another's, and
    // neither does a string this service never minted.
    let (_, first) = get_api_mappings(&emulator, DOMAIN, "?maxResults=1").await;
    let issued = as_str(&first, "nextToken");
    assert!(!issued.is_empty(), "expected a NextToken, got {first}");
    let (_, neighbour_first) =
        get_api_mappings(&emulator, "neighbour.example.com", "?maxResults=1").await;
    let foreign = as_str(&neighbour_first, "nextToken");

    let mut cases: Vec<(&str, String)> = vec![
        ("a word", "not-a-token".to_string()),
        ("an empty token", String::new()),
        (
            "an issued token with its last character dropped",
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "a token far longer than any this service mints",
            "ab".repeat(4096),
        ),
    ];
    if !foreign.is_empty() {
        cases.push(("a token issued for another domain name's mappings", foreign));
    }
    for (label, token) in &cases {
        let (status, body) =
            get_api_mappings(&emulator, DOMAIN, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from GetApiMappings, got {status} {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {label:?}: expected the pagination-cursor refusal message, got {body}"
        );
    }

    // The positive control: the token this reader DID issue resumes the walk.
    let (status, resumed) =
        get_api_mappings(&emulator, DOMAIN, &format!("?maxResults=1&nextToken={issued}")).await;
    assert_eq!(
        status, 200,
        "expected the issued token to resume the walk, got {status} {resumed}"
    );
    assert_eq!(
        resumed["items"].as_array().map(Vec::len),
        Some(1),
        "expected the next page to hold one mapping, got {resumed}"
    );
}
