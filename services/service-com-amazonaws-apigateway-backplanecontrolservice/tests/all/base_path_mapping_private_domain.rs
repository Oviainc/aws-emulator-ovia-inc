// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which custom domain name record a base path mapping belongs to:
//! `CreateBasePathMapping` and `GetBasePathMapping` addressed by `domainName` **and**
//! `domainNameId`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULE under test: a base path mapping is a record of the ONE custom domain name the
//! request's `domainName` + `domainNameId` pair resolves to — the same pair the domain
//! name's own readers resolve a record by, where an id identifies a private record and an
//! edge-optimized or regional record has none — so a base path is unique per RESOLVED
//! record rather than per name, each record reports only its own mappings, and a pair that
//! resolves to no stored domain name is refused with that record's missing-resource
//! `NotFoundException` and stores nothing.
//!
//! The rows span what the rule generalises over rather than repeating one call: how the
//! pair is spelled (a private record with its id, its same-named sibling with another id, a
//! regional record with no id, a pair whose id is absent, unknown or belongs to no record),
//! and the forms a base path takes under each (a plain path and the `(none)` spelling).
//!
//! The sibling `base_path_mapping` suite covers the base-path spellings, the stage and the
//! quota on a single domain name; this one covers only the identifier pair, so the two do
//! not assert the same rows twice.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`create_api`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `created_domain` and `create_deployed_stage` are the domain-name and stage
//! suites' one spelling of those creates.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    BASE_PATH_MAPPING_ALREADY_EXISTS, NONE_BASE_PATH, NO_SUCH_BASE_PATH_MAPPING,
    NO_SUCH_DOMAIN_NAME, PRIVATE_ENDPOINT_TYPE, REGIONAL_ENDPOINT_TYPE,
};

use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

const NOT_FOUND: (u16, &str) = (404, "NotFoundException");
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// One custom domain name record, as a request addresses it: its name and the
/// `domainNameId` a private record is told apart by (`None` for a record that has none).
#[derive(Clone, Debug)]
struct DomainRef {
    name: String,
    id: Option<String>,
}

impl DomainRef {
    /// The `domainNameId` query string this record is addressed by, empty for a record
    /// addressed by its name alone.
    fn query(&self) -> String {
        match &self.id {
            Some(id) => format!("?domainNameId={id}"),
            None => String::new(),
        }
    }
}

/// `CreateBasePathMapping` under the domain name `domain` addresses, with `body` as given.
async fn create_mapping(gateway: &Gateway, domain: &DomainRef, body: Value) -> Answer {
    let uri = format!(
        "/domainnames/{}/basepathmappings{}",
        domain.name,
        domain.query()
    );
    gateway.call("POST", &uri, Some(body)).await
}

/// `GetBasePathMapping` for `base_path` of the domain name `domain` addresses.
async fn get_mapping(gateway: &Gateway, domain: &DomainRef, base_path: &str) -> Answer {
    let uri = format!(
        "/domainnames/{}/basepathmappings/{base_path}{}",
        domain.name,
        domain.query()
    );
    gateway.call("GET", &uri, None).await
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
/// `None`.
fn mapping_of(body: &Value, case: &str) -> (String, String, Option<String>) {
    (
        string_member(body, "basePath", case),
        string_member(body, "restApiId", case),
        body["stage"].as_str().map(str::to_owned),
    )
}

/// A PRIVATE custom domain name called `name`, answering the record's identifier pair. A
/// `domainNameId` is supported only for a private record, and AWS lets several private
/// records carry one name, so this is the create the identifier pair is observable through.
async fn created_private_domain(gateway: &Gateway, name: &str, case: &str) -> DomainRef {
    let created = created_domain(
        gateway,
        name,
        json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
    )
    .await;
    DomainRef {
        name: name.to_string(),
        id: Some(string_member(&created, "domainNameId", case)),
    }
}

/// A REGIONAL custom domain name called `name`: a record addressed by its name alone,
/// because only a private one carries a `domainNameId`.
async fn created_regional_domain(gateway: &Gateway, name: &str) -> DomainRef {
    let created = created_domain(
        gateway,
        name,
        json!({ "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] } }),
    )
    .await;
    assert!(
        created["domainNameId"].is_null(),
        "case the regional record {name}: expected no domainNameId, got {:?}",
        created["domainNameId"]
    );
    DomainRef {
        name: name.to_string(),
        id: None,
    }
}

/// An API with one deployed stage, answering `(restApiId, stageName)` — what a mapping
/// points at.
async fn deployed_api(gateway: &Gateway, name: &str, stage: &str) -> (String, String) {
    let (api, _root) = create_api(gateway, name).await;
    create_deployed_stage(gateway, &api, stage).await;
    (api, stage.to_string())
}

#[tokio::test]
async fn a_mapping_is_a_record_of_the_domain_name_the_identifier_pair_resolves_to() {
    let gateway = Gateway::new();
    let (first_api, first_stage) = deployed_api(&gateway, "private-first", "prod").await;
    let (second_api, second_stage) = deployed_api(&gateway, "private-second", "beta").await;
    let (regional_api, regional_stage) = deployed_api(&gateway, "regional-api", "gamma").await;

    // TWO private records carrying ONE name, told apart by their ids, plus a regional
    // record addressed by its name alone: the three spellings of the identifier pair.
    let shared_name = "shared.example.com";
    let first = created_private_domain(&gateway, shared_name, "the first private record").await;
    let second = created_private_domain(&gateway, shared_name, "the second private record").await;
    assert_ne!(
        first.id, second.id,
        "case two private records of one name: expected distinct domainNameIds, got {:?} twice",
        first.id
    );
    let regional = created_regional_domain(&gateway, "regional.example.com").await;

    // One base path, created under each record: the rows differ in WHICH record the pair
    // resolves to, and each points at its own API and stage so a mapping reported under the
    // wrong record is visible.
    let rows = [
        (
            "the first private record",
            &first,
            &first_api,
            &first_stage,
        ),
        (
            "its same-named sibling",
            &second,
            &second_api,
            &second_stage,
        ),
        (
            "the regional record addressed by name alone",
            &regional,
            &regional_api,
            &regional_stage,
        ),
    ];

    for (case, domain, api, stage) in rows {
        let created = create_mapping(
            &gateway,
            domain,
            json!({ "basePath": "v1", "restApiId": api, "stage": stage }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            mapping_of(&created.body, case),
            ("v1".to_string(), api.to_string(), Some(stage.to_string())),
            "case {case}: expected the create to report the record it stored, got {:?}",
            created.body
        );
    }

    // Each record reports its OWN mapping at that base path, and a second read reports the
    // same body: resolving the pair changes nothing a later caller sees.
    for (case, domain, api, stage) in rows {
        let read = get_mapping(&gateway, domain, "v1").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetBasePathMapping, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            mapping_of(&read.body, case),
            ("v1".to_string(), api.to_string(), Some(stage.to_string())),
            "case {case}: expected the mapping of this record only, got {:?}",
            read.body
        );
        let again = get_mapping(&gateway, domain, "v1").await;
        assert_eq!(
            again.body, read.body,
            "case {case}: expected a second read to report the same record, got {:?} then {:?}",
            read.body, again.body
        );
    }
}

#[tokio::test]
async fn a_base_path_is_unique_per_resolved_record_rather_than_per_domain_name() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "uniqueness-api", "prod").await;
    let shared_name = "unique.example.com";
    let first = created_private_domain(&gateway, shared_name, "the first private record").await;
    let second = created_private_domain(&gateway, shared_name, "the second private record").await;

    // The rows differ in the FORM of the base path — a plain one, and the `(none)` a
    // request spells by naming no base path at all — because the uniqueness rule is the
    // same for both.
    for (case, requested, stored) in [
        ("a plain base path", Some("v1"), "v1"),
        ("no basePath at all", None, NONE_BASE_PATH),
    ] {
        let mut request = json!({ "restApiId": api, "stage": stage });
        if let Some(requested) = requested {
            request["basePath"] = json!(requested);
        }

        // The SAME base path under each of the two same-named records is two mappings: the
        // pair, not the name, selects the record it is unique within.
        for (record_case, domain) in [("the first record", &first), ("its sibling", &second)] {
            let created = create_mapping(&gateway, domain, request.clone()).await;
            assert_eq!(
                created.status, 201,
                "case {case} under {record_case}: expected 201, got {} with {:?}",
                created.status, created.body
            );
            assert_eq!(
                mapping_of(&created.body, case).0,
                stored.to_string(),
                "case {case} under {record_case}: expected the stored base path {stored:?}, got {:?}",
                created.body
            );
        }

        // A SECOND create of that base path under the same record is the modelled conflict,
        // on each record independently...
        for (record_case, domain) in [("the first record", &first), ("its sibling", &second)] {
            let refused = create_mapping(&gateway, domain, request.clone()).await;
            assert_refusal(
                &refused,
                CONFLICT,
                BASE_PATH_MAPPING_ALREADY_EXISTS,
                &format!("{case} again under {record_case}"),
            );
        }

        // ...and the refused creates left each record's mapping exactly as its accepted
        // create stored it.
        for (record_case, domain) in [("the first record", &first), ("its sibling", &second)] {
            let read = get_mapping(&gateway, domain, stored).await;
            assert_eq!(
                mapping_of(&read.body, case),
                (
                    stored.to_string(),
                    api.clone(),
                    Some(stage.clone())
                ),
                "case {case} under {record_case}, after the refused create: expected the stored mapping, got {} with {:?}",
                read.status,
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_pair_no_domain_name_answers_to_stores_nothing_and_is_refused() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "refusal-api", "prod").await;
    let private_name = "pairs.example.com";
    let private = created_private_domain(&gateway, private_name, "the private record").await;
    let private_id = private
        .id
        .clone()
        .expect("a private record is created with a domainNameId");
    let regional = created_regional_domain(&gateway, "pairs-regional.example.com").await;
    for (case, domain) in [("the private record", &private), ("the regional record", &regional)] {
        let created = create_mapping(
            &gateway,
            domain,
            json!({ "basePath": "v1", "restApiId": api, "stage": stage }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            created.status, created.body
        );
    }

    // Every pair here resolves to NO stored domain name, however near it is to one that
    // exists: the private record's name without its id, with an id no record carries, with
    // its own id extended by a character, the regional record's name WITH an id (it has
    // none), and an unknown name carrying the private record's id.
    let pairs = [
        (
            "the private name with no domainNameId",
            DomainRef {
                name: private_name.to_string(),
                id: None,
            },
        ),
        (
            "the private name with an unknown domainNameId",
            DomainRef {
                name: private_name.to_string(),
                id: Some("zzzzz99999".to_string()),
            },
        ),
        (
            "the private name with its own id extended",
            DomainRef {
                name: private_name.to_string(),
                id: Some(format!("{private_id}x")),
            },
        ),
        (
            "the regional name carrying a domainNameId",
            DomainRef {
                name: regional.name.clone(),
                id: Some(private_id.clone()),
            },
        ),
        (
            "an unknown name carrying the private record's id",
            DomainRef {
                name: "absent.example.com".to_string(),
                id: Some(private_id.clone()),
            },
        ),
    ];

    for (case, domain) in &pairs {
        // The create is refused with the DOMAIN NAME's missing-resource message, because
        // the record a mapping would belong to is what the pair failed to resolve.
        let refused = create_mapping(
            &gateway,
            domain,
            json!({ "basePath": "ghost", "restApiId": api, "stage": stage }),
        )
        .await;
        assert_refusal(
            &refused,
            NOT_FOUND,
            NO_SUCH_DOMAIN_NAME,
            &format!("{case}, on the create"),
        );
        // The read of the same pair is refused the same way, at a base path that exists
        // under the record the pair nearly named.
        let read = get_mapping(&gateway, domain, "v1").await;
        assert_refusal(&read, NOT_FOUND, NO_SUCH_DOMAIN_NAME, case);
    }

    // The refused creates persisted nothing: the base path they all named resolves to no
    // mapping under either stored record, while the mapping each record does hold is
    // unchanged.
    for (case, domain) in [("the private record", &private), ("the regional record", &regional)] {
        let ghost = get_mapping(&gateway, domain, "ghost").await;
        assert_refusal(
            &ghost,
            NOT_FOUND,
            NO_SUCH_BASE_PATH_MAPPING,
            &format!("the refused base path under {case}"),
        );
        let read = get_mapping(&gateway, domain, "v1").await;
        assert_eq!(
            mapping_of(&read.body, case),
            ("v1".to_string(), api.clone(), Some(stage.clone())),
            "case {case}, after the refused creates: expected the stored mapping, got {} with {:?}",
            read.status,
            read.body
        );
    }
}
