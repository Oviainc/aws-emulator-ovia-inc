// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a custom domain name's base path mappings:
//! `CreateBasePathMapping` and `GetBasePathMapping`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULE under test: a base path mapping is a record OF the custom domain name it was
//! created against, keyed by its base path (with an absent or empty one stored and
//! addressed as the `(none)` the member documents), so `GetBasePathMapping` reports the
//! mapping the request's `domainName` + `basePath` pair resolves to, whatever the base
//! path's spelling and whether or not the mapping names a stage; a pair that resolves to
//! no stored mapping is the modelled `NotFoundException`, and nothing a read does changes
//! what a later caller sees.
//!
//! The rows span what the rule generalises over rather than repeating one recorded call:
//! the forms a base path takes (absent, `(none)`, a plain path, one with digits, hyphens
//! and underscores, one at the published length of a stage name), a mapping with a stage
//! and one without, the identifiers both operations resolve over the records that do and
//! do not exist, and the per-domain-name quota at its bound and one past it.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`create_api`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `created_domain` and `create_deployed_stage` are the domain-name and stage
//! suites' one spelling of those creates.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    BASE_PATH_MAPPING_ALREADY_EXISTS, INVALID_REST_API_IDENTIFIER,
    MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME, NONE_BASE_PATH, NO_SUCH_BASE_PATH_MAPPING,
    NO_SUCH_DOMAIN_NAME, NO_SUCH_STAGE,
};

use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");
const CONFLICT: (u16, &str) = (409, "ConflictException");
const LIMIT_EXCEEDED: (u16, &str) = (429, "LimitExceededException");

/// `CreateBasePathMapping` under `domain_name` with `body` sent as given.
async fn create_mapping(gateway: &Gateway, domain_name: &str, body: Value) -> Answer {
    gateway
        .call(
            "POST",
            &format!("/domainnames/{domain_name}/basepathmappings"),
            Some(body),
        )
        .await
}

/// `GetBasePathMapping` for `base_path` of `domain_name`.
async fn get_mapping(gateway: &Gateway, domain_name: &str, base_path: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/domainnames/{domain_name}/basepathmappings/{base_path}"),
            None,
        )
        .await
}

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// The `basePath`, `restApiId` and `stage` a body reports, with an absent member as
/// `None`, which is what a mapping that names no stage must report.
fn mapping_of(body: &Value, case: &str) -> (String, String, Option<String>) {
    (
        string_member(body, "basePath", case),
        string_member(body, "restApiId", case),
        body["stage"].as_str().map(str::to_owned),
    )
}

/// An API with one deployed stage, answering `(restApiId, stageName)` — what a mapping
/// points at.
async fn deployed_api(gateway: &Gateway, name: &str, stage: &str) -> (String, String) {
    let (api, _root) = create_api(gateway, name).await;
    create_deployed_stage(gateway, &api, stage).await;
    (api, stage.to_string())
}

/// One row of the base-path table: the case, the `basePath` the request carries (`None`
/// for a request that carries none), whether it names the stage, and the base path the
/// stored mapping must be reported and addressed by.
struct BasePathRow {
    case: &'static str,
    requested: Option<&'static str>,
    with_stage: bool,
    stored: &'static str,
}

#[tokio::test]
async fn a_created_base_path_mapping_is_reported_by_the_base_path_it_is_addressed_by() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "mapped-api", "prod").await;

    // `MAX_STAGE_NAME_CHARS`-free rows: what varies is the SPELLING of the base path and
    // whether the mapping names a stage. Each row gets its own domain name, because the
    // base path is unique per domain name and a row's spelling may collide with another's
    // (an absent base path and an explicit `(none)` are the SAME base path).
    let rows = [
        BasePathRow {
            case: "no basePath at all",
            requested: None,
            with_stage: true,
            stored: NONE_BASE_PATH,
        },
        BasePathRow {
            case: "an empty basePath",
            requested: Some(""),
            with_stage: true,
            stored: NONE_BASE_PATH,
        },
        BasePathRow {
            case: "the documented (none) spelling",
            requested: Some(NONE_BASE_PATH),
            with_stage: true,
            stored: NONE_BASE_PATH,
        },
        BasePathRow {
            case: "a plain base path",
            requested: Some("v1"),
            with_stage: true,
            stored: "v1",
        },
        BasePathRow {
            case: "a base path with digits, hyphens and underscores",
            requested: Some("Orders-2_beta"),
            with_stage: true,
            stored: "Orders-2_beta",
        },
        BasePathRow {
            case: "a base path that names no stage",
            requested: Some("stageless"),
            with_stage: false,
            stored: "stageless",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let domain_name = format!("mapping-{index}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;

        let mut request = json!({ "restApiId": api });
        if let Some(requested) = row.requested {
            request["basePath"] = json!(requested);
        }
        if row.with_stage {
            request["stage"] = json!(stage);
        }
        let created = create_mapping(&gateway, &domain_name, request).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            row.case, created.status, created.body
        );

        let expected_stage = row.with_stage.then(|| stage.clone());
        assert_eq!(
            mapping_of(&created.body, row.case),
            (row.stored.to_string(), api.clone(), expected_stage.clone()),
            "case {}: expected the create to report the record it stored, got {:?}",
            row.case,
            created.body
        );

        // The read reports the record the create stored, addressed by the base path the
        // record carries — which is the `(none)` spelling for a request that named no base
        // path at all.
        let read = get_mapping(&gateway, &domain_name, row.stored).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected 200 from GetBasePathMapping({}), got {} with {:?}",
            row.case, row.stored, read.status, read.body
        );
        assert_eq!(
            mapping_of(&read.body, row.case),
            (row.stored.to_string(), api.clone(), expected_stage.clone()),
            "case {}: expected the stored mapping, got {:?}",
            row.case,
            read.body
        );

        // A read changes nothing a later caller sees: the next read of the same record
        // reports exactly the same body.
        let again = get_mapping(&gateway, &domain_name, row.stored).await;
        assert_eq!(
            again.body, read.body,
            "case {}: expected a second read to report the same record, got {:?} then {:?}",
            row.case, read.body, again.body
        );
    }
}

#[tokio::test]
async fn a_base_path_is_unique_per_domain_name_and_a_refused_create_stores_nothing() {
    let gateway = Gateway::new();
    let (first_api, first_stage) = deployed_api(&gateway, "first-api", "prod").await;
    let (second_api, second_stage) = deployed_api(&gateway, "second-api", "beta").await;

    // The same base path under TWO domain names is two mappings: the uniqueness rule is
    // per domain name, because the mapping is a record of the domain name.
    for (case, domain_name, api, stage) in [
        ("the first domain name", "one.example.com", &first_api, &first_stage),
        (
            "the second domain name",
            "two.example.com",
            &second_api,
            &second_stage,
        ),
    ] {
        created_domain(&gateway, domain_name, json!({})).await;
        let created = create_mapping(
            &gateway,
            domain_name,
            json!({ "basePath": "shared", "restApiId": api, "stage": stage }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            created.status, created.body
        );
        let read = get_mapping(&gateway, domain_name, "shared").await;
        assert_eq!(
            mapping_of(&read.body, case),
            ("shared".to_string(), api.to_string(), Some(stage.to_string())),
            "case {case}: expected each domain name to report its own mapping, got {:?}",
            read.body
        );
    }

    // A SECOND create of one base path under one domain name is the modelled conflict,
    // whatever the second request would have mapped it to...
    for (case, body) in [
        (
            "the same base path at the same API",
            json!({ "basePath": "shared", "restApiId": first_api, "stage": first_stage }),
        ),
        (
            "the same base path at another API",
            json!({ "basePath": "shared", "restApiId": second_api, "stage": second_stage }),
        ),
        (
            "the same base path with no stage",
            json!({ "basePath": "shared", "restApiId": second_api }),
        ),
    ] {
        let refused = create_mapping(&gateway, "one.example.com", body).await;
        assert_refusal(&refused, CONFLICT, BASE_PATH_MAPPING_ALREADY_EXISTS, case);
    }

    // ...and the refused creates left the stored mapping exactly as the accepted one
    // stored it.
    let read = get_mapping(&gateway, "one.example.com", "shared").await;
    assert_eq!(
        mapping_of(&read.body, "after the refused creates"),
        (
            "shared".to_string(),
            first_api.clone(),
            Some(first_stage.clone())
        ),
        "case after the refused creates: expected the first mapping, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn an_identifier_no_record_answers_to_is_refused_with_the_modelled_failure() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "refusal-api", "prod").await;
    created_domain(&gateway, "refusals.example.com", json!({})).await;
    let created = create_mapping(
        &gateway,
        "refusals.example.com",
        json!({ "basePath": "v1", "restApiId": api, "stage": stage }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the mapping under test: expected 201, got {} with {:?}",
        created.status, created.body
    );

    // A `domainName` no stored record answers to is the domain name's own missing-resource
    // refusal, on BOTH operations and whatever base path the request names.
    for (case, domain_name, base_path) in [
        ("an unknown domain name", "absent.example.com", "v1"),
        (
            "an unknown domain name, at the (none) base path",
            "absent.example.com",
            NONE_BASE_PATH,
        ),
        (
            "a domain name that differs in one label",
            "refusals.example.org",
            "v1",
        ),
    ] {
        let read = get_mapping(&gateway, domain_name, base_path).await;
        assert_refusal(&read, NOT_FOUND, NO_SUCH_DOMAIN_NAME, case);
        let refused = create_mapping(
            &gateway,
            domain_name,
            json!({ "basePath": base_path, "restApiId": api, "stage": stage }),
        )
        .await;
        assert_refusal(
            &refused,
            NOT_FOUND,
            NO_SUCH_DOMAIN_NAME,
            &format!("{case}, on the create"),
        );
    }

    // A `basePath` the stored domain name carries no mapping for resolves to nothing,
    // whatever its spelling — the `(none)` base path included, which this domain name has
    // no mapping at.
    for (case, base_path) in [
        ("an unmapped base path", "v2"),
        ("the (none) base path with nothing mapped at it", NONE_BASE_PATH),
        ("a base path differing in case", "V1"),
        ("a base path that extends a mapped one", "v1x"),
    ] {
        let read = get_mapping(&gateway, "refusals.example.com", base_path).await;
        assert_refusal(&read, NOT_FOUND, NO_SUCH_BASE_PATH_MAPPING, case);
    }

    // A `restApiId` that resolves to no stored API is this create's BAD REQUEST: the
    // mapping would point at an API that does not exist.
    for (case, rest_api_id) in [
        ("an unknown restApiId", "nonexist12"),
        ("a restApiId of the wrong shape", "not an api"),
    ] {
        let refused = create_mapping(
            &gateway,
            "refusals.example.com",
            json!({ "basePath": "fresh", "restApiId": rest_api_id, "stage": stage }),
        )
        .await;
        assert_refusal(&refused, BAD_REQUEST, INVALID_REST_API_IDENTIFIER, case);
    }

    // A `stage` the named API has no stage for is refused the same way; the `(none)`
    // spelling names NO stage at all, so it is not resolved against the API's stages.
    for (case, requested_stage) in [
        ("an unknown stage", "absent"),
        ("a stage differing in case", "PROD"),
    ] {
        let refused = create_mapping(
            &gateway,
            "refusals.example.com",
            json!({ "basePath": "fresh", "restApiId": api, "stage": requested_stage }),
        )
        .await;
        assert_refusal(&refused, BAD_REQUEST, NO_SUCH_STAGE, case);
    }
    let accepted = create_mapping(
        &gateway,
        "refusals.example.com",
        json!({ "basePath": "fresh", "restApiId": api, "stage": NONE_BASE_PATH }),
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case the (none) stage: expected 201, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        mapping_of(&accepted.body, "the (none) stage"),
        (
            "fresh".to_string(),
            api.clone(),
            Some(NONE_BASE_PATH.to_string())
        ),
        "case the (none) stage: expected the stored mapping, got {:?}",
        accepted.body
    );

    // Every refused create persisted nothing: the base path they all named resolves to
    // the one mapping the accepted create stored, and nothing else was added.
    let read = get_mapping(&gateway, "refusals.example.com", "fresh").await;
    assert_eq!(
        mapping_of(&read.body, "after the refused creates"),
        (
            "fresh".to_string(),
            api.clone(),
            Some(NONE_BASE_PATH.to_string())
        ),
        "case after the refused creates: expected the accepted mapping, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_domain_name_holds_at_most_its_quota_of_base_path_mappings() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "quota-api", "prod").await;
    created_domain(&gateway, "quota.example.com", json!({})).await;

    // Fill the domain name to API Gateway's published quota; the LAST mapping that fits is
    // still stored.
    for index in 0..MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME {
        let answer = create_mapping(
            &gateway,
            "quota.example.com",
            json!({ "basePath": format!("path-{index}"), "restApiId": api, "stage": stage }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case create {index} of {MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME} (at the quota): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // ONE PAST the bound: refused with the modelled failure, whatever shape the request
    // takes.
    for (case, body) in [
        (
            "one past the quota",
            json!({ "basePath": "one-past", "restApiId": api, "stage": stage }),
        ),
        (
            "one past the quota, with no stage",
            json!({ "basePath": "one-past-stageless", "restApiId": api }),
        ),
        (
            "one past the quota, at the (none) base path",
            json!({ "restApiId": api, "stage": stage }),
        ),
    ] {
        let refused = create_mapping(&gateway, "quota.example.com", body).await;
        assert_failure(&refused, LIMIT_EXCEEDED.0, LIMIT_EXCEEDED.1, case);
    }

    // A refused create changes nothing a later caller sees: the mapping at the bound still
    // reads, and each refused base path resolves to no mapping.
    let at_the_bound = format!("path-{}", MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME - 1);
    let last = get_mapping(&gateway, "quota.example.com", &at_the_bound).await;
    assert_eq!(
        mapping_of(&last.body, "the mapping at the quota"),
        (at_the_bound.clone(), api.clone(), Some(stage.clone())),
        "case the mapping at the quota: expected it to still be readable, got {} with {:?}",
        last.status,
        last.body
    );
    for base_path in ["one-past", "one-past-stageless", NONE_BASE_PATH] {
        let refused = get_mapping(&gateway, "quota.example.com", base_path).await;
        assert_refusal(
            &refused,
            NOT_FOUND,
            NO_SUCH_BASE_PATH_MAPPING,
            &format!("the refused base path {base_path}"),
        );
    }
}
