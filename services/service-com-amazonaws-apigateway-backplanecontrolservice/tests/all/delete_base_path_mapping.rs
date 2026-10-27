// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteBasePathMapping`, read back through the operation that
//! reports the record it removes (`GetBasePathMapping`) and the create that re-uses the
//! base path it frees.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a delete removes exactly the one mapping its identifiers address — the custom domain
//!   name record the `domainName` + `domainNameId` pair resolves to, and the `basePath`
//!   under the spelling the record was stored with, where an absent base path is the
//!   `(none)` the member documents — so the read that reported that mapping stops
//!   reporting it while every sibling mapping, every other domain name record and the
//!   domain name itself are untouched, and the base path it held becomes free to create
//!   again (its place in the per-domain-name quota included);
//! * identifiers no stored record answers to are the modelled `NotFoundException` (404),
//!   with the domain name's own sentence when the pair resolves to no record and the
//!   mapping's when the record holds no mapping at that base path, rather than a silent
//!   success — which is what makes a second delete of one mapping resolve to one success
//!   and one 404 — and a refused delete removes nothing.
//!
//! None of those answers is a 501: every request a caller can make against this operation
//! is one of the statuses its model declares
//! (`smithy_query(query="operation", operation="DeleteBasePathMapping")`).
//!
//! [`Gateway`], [`Answer`] and [`assert_failure`] are this crate's one wire-test harness,
//! reused rather than standing a second emulator up another way; [`DomainRef`],
//! [`create_mapping`], [`get_mapping`], [`mapping_of`], [`created_private_domain`],
//! [`created_regional_domain`] and [`deployed_api`] are the `base_path_mapping_private_domain`
//! suite's one spelling of a domain name record and of the calls that create and read a
//! mapping of it, so this suite adds only the delete.

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME, NONE_BASE_PATH, NO_SUCH_BASE_PATH_MAPPING,
    NO_SUCH_DOMAIN_NAME,
};

use crate::base_path_mapping_private_domain::{
    create_mapping, created_private_domain, created_regional_domain, deployed_api, get_mapping,
    mapping_of, DomainRef,
};
use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};

const NOT_FOUND: &str = "NotFoundException";
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The status `DeleteBasePathMapping`'s model declares for a success
/// (`smithy_query(query="operation", operation="DeleteBasePathMapping")`:
/// `DELETE /domainnames/{domainName}/basepathmappings/{basePath} -> 202`).
const DELETED: u16 = 202;

/// The key that tells one stored domain name record apart from every other, for a case
/// name: the pair of identifiers that address it, which is the name alone for a record
/// that carries no `domainNameId`.
fn record_key(domain: &DomainRef) -> String {
    match &domain.id {
        Some(id) => format!("{}+{id}", domain.name),
        None => domain.name.clone(),
    }
}

/// `DeleteBasePathMapping` for `base_path` of the record `domain` addresses.
async fn delete_mapping(gateway: &Gateway, domain: &DomainRef, base_path: &str) -> Answer {
    let uri = format!(
        "/domainnames/{}/basepathmappings/{base_path}{}",
        domain.name,
        domain.query()
    );
    gateway.call("DELETE", &uri, None).await
}

/// `GetDomainName` for the record `domain` addresses.
async fn get_domain(gateway: &Gateway, domain: &DomainRef) -> Answer {
    let uri = format!("/domainnames/{}{}", domain.name, domain.query());
    gateway.call("GET", &uri, None).await
}

/// Assert a delete succeeded with the status its model declares and an empty body.
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, DELETED,
        "case {case}: expected {DELETED}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null(),
        "case {case}: expected an empty body, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the modelled 404 carrying exactly `message`.
fn assert_not_found(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, 404, NOT_FOUND, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// One mapping this suite created: how the case is named, the base path the record is
/// stored and addressed under, and the stage it names.
#[derive(Clone, Debug)]
struct Mapping {
    case: String,
    stored: String,
    stage: Option<String>,
}

/// Create one mapping under `domain` and answer how it is addressed, asserting the create
/// was accepted and reported the record it stored.
async fn stored_mapping(
    gateway: &Gateway,
    domain: &DomainRef,
    case: &str,
    requested: Option<&str>,
    stored: &str,
    api: &str,
    stage: Option<&str>,
) -> Mapping {
    let mut request = json!({ "restApiId": api });
    if let Some(requested) = requested {
        request["basePath"] = json!(requested);
    }
    if let Some(stage) = stage {
        request["stage"] = json!(stage);
    }
    let created = create_mapping(gateway, domain, request).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        mapping_of(&created.body, case),
        (
            stored.to_string(),
            api.to_string(),
            stage.map(str::to_owned)
        ),
        "case {case}: expected the create to report the record it stored, got {:?}",
        created.body
    );
    Mapping {
        case: case.to_owned(),
        stored: stored.to_owned(),
        stage: stage.map(str::to_owned),
    }
}

#[tokio::test]
async fn deleting_a_base_path_mapping_removes_that_mapping_alone_and_frees_its_base_path() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "delete-mapping-api", "prod").await;
    let domain = created_regional_domain(&gateway, "deletes.example.com").await;
    // A second record, so a delete that reached the wrong domain name's mappings is
    // visible: it holds a mapping at a base path the first record also holds.
    let other = created_regional_domain(&gateway, "untouched.example.com").await;

    // The rows differ along what the rule generalises over: the SPELLING of the base path
    // the delete addresses (one a request carried as nothing at all, the documented
    // `(none)`, a plain path, one with digits, hyphens and underscores, one that extends
    // another row's, and a long one), and whether the mapping names a stage. An absent
    // base path and an explicit `(none)` are the SAME base path, so only one row spells it
    // either way.
    let long_base_path = "p".repeat(64);
    let rows: Vec<(&str, Option<&str>, String, Option<&str>)> = vec![
        (
            "no basePath at all, addressed as (none)",
            None,
            NONE_BASE_PATH.to_string(),
            Some(stage.as_str()),
        ),
        (
            "a plain base path",
            Some("v1"),
            "v1".to_string(),
            Some(stage.as_str()),
        ),
        (
            "a base path that extends another",
            Some("v1x"),
            "v1x".to_string(),
            Some(stage.as_str()),
        ),
        (
            "a base path with digits, hyphens and underscores",
            Some("Orders-2_beta"),
            "Orders-2_beta".to_string(),
            Some(stage.as_str()),
        ),
        (
            "a long base path",
            Some(long_base_path.as_str()),
            long_base_path.clone(),
            Some(stage.as_str()),
        ),
        (
            "a mapping that names no stage",
            Some("stageless"),
            "stageless".to_string(),
            None,
        ),
    ];

    let mut remaining: Vec<Mapping> = Vec::new();
    for (case, requested, stored, row_stage) in &rows {
        remaining.push(
            stored_mapping(&gateway, &domain, case, *requested, stored, &api, *row_stage).await,
        );
    }
    // The other record's mapping is at a base path the record under test also holds.
    let elsewhere = stored_mapping(
        &gateway,
        &other,
        "the other record's mapping",
        Some("v1"),
        "v1",
        &api,
        Some(stage.as_str()),
    )
    .await;

    // Each mapping is deleted in turn: the delete is accepted, the read stops reporting
    // it, a second delete of the same identifiers is the modelled not-found, and every
    // mapping that is left — here and under the other record — still reads exactly as its
    // create stored it.
    while !remaining.is_empty() {
        let target = remaining.remove(0);
        let case = &target.case;

        let answer = delete_mapping(&gateway, &domain, &target.stored).await;
        assert_deleted(&answer, case);

        let read = get_mapping(&gateway, &domain, &target.stored).await;
        assert_not_found(
            &read,
            NO_SUCH_BASE_PATH_MAPPING,
            &format!("GetBasePathMapping after {case}"),
        );

        let again = delete_mapping(&gateway, &domain, &target.stored).await;
        assert_not_found(
            &again,
            NO_SUCH_BASE_PATH_MAPPING,
            &format!("a second DeleteBasePathMapping of {case}"),
        );

        // The record the mappings belong to is itself untouched by a delete of one of
        // them.
        let domain_read = get_domain(&gateway, &domain).await;
        assert_eq!(
            domain_read.status,
            200,
            "case GetDomainName({}) after {case}: expected the record to still read, got {} \
             with {:?}",
            record_key(&domain),
            domain_read.status,
            domain_read.body
        );

        for sibling in remaining.iter().chain(std::iter::once(&elsewhere)) {
            let under = if sibling.case == elsewhere.case {
                &other
            } else {
                &domain
            };
            let still_there = get_mapping(&gateway, under, &sibling.stored).await;
            assert_eq!(
                mapping_of(&still_there.body, &sibling.case),
                (
                    sibling.stored.clone(),
                    api.clone(),
                    sibling.stage.clone()
                ),
                "case {} after {case}: expected the sibling mapping to still read, got {} \
                 with {:?}",
                sibling.case,
                still_there.status,
                still_there.body
            );
        }
    }

    // The base path a delete freed is free to create again, under either spelling, and the
    // record the new create stores is what a later read reports.
    for (case, requested, stored) in [
        ("the (none) base path, spelled explicitly", Some(NONE_BASE_PATH), NONE_BASE_PATH),
        ("a base path a delete freed", Some("v1"), "v1"),
    ] {
        stored_mapping(
            &gateway,
            &domain,
            &format!("re-creating {case}"),
            requested,
            stored,
            &api,
            Some(stage.as_str()),
        )
        .await;
        let read = get_mapping(&gateway, &domain, stored).await;
        assert_eq!(
            mapping_of(&read.body, case),
            (stored.to_string(), api.clone(), Some(stage.clone())),
            "case re-creating {case}: expected the re-created mapping to read, got {} with {:?}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn a_delete_removes_the_mapping_of_the_record_its_identifier_pair_resolves_to() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "pair-delete-api", "prod").await;

    // Two PRIVATE records carrying ONE name, told apart by their ids, plus a regional
    // record addressed by its name alone: the spellings of the identifier pair a delete
    // resolves a record by. Each holds a mapping at the same base path, so a delete that
    // resolved the wrong record is visible.
    let shared_name = "shared-delete.example.com";
    let first = created_private_domain(&gateway, shared_name, "the first private record").await;
    let second = created_private_domain(&gateway, shared_name, "the second private record").await;
    assert_ne!(
        first.id, second.id,
        "case two private records of one name: expected distinct domainNameIds, got {:?} twice",
        first.id
    );
    let regional = created_regional_domain(&gateway, "regional-delete.example.com").await;

    let records = [
        ("the first private record", &first),
        ("its same-named sibling", &second),
        ("the regional record addressed by name alone", &regional),
    ];
    for (case, domain) in records {
        stored_mapping(
            &gateway,
            domain,
            case,
            Some("v1"),
            "v1",
            &api,
            Some(stage.as_str()),
        )
        .await;
    }

    // Deleting under one pair removes that record's mapping and no other's: the pair, not
    // the name, selects the record the base path is unique within.
    let mut left: Vec<(&str, &DomainRef)> = records.to_vec();
    while !left.is_empty() {
        let (case, target) = left.remove(0);
        assert_deleted(&delete_mapping(&gateway, target, "v1").await, case);
        assert_not_found(
            &get_mapping(&gateway, target, "v1").await,
            NO_SUCH_BASE_PATH_MAPPING,
            &format!("GetBasePathMapping({}) after {case}", record_key(target)),
        );
        for (sibling_case, sibling) in &left {
            let read = get_mapping(&gateway, sibling, "v1").await;
            assert_eq!(
                mapping_of(&read.body, sibling_case),
                ("v1".to_string(), api.clone(), Some(stage.clone())),
                "case {sibling_case} after {case}: expected the mapping of {} to still read, \
                 got {} with {:?}",
                record_key(sibling),
                read.status,
                read.body
            );
        }
    }
}

#[tokio::test]
async fn identifiers_no_record_answers_to_are_the_modelled_not_found_and_remove_nothing() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "refusal-delete-api", "prod").await;
    let private_name = "refusal-private.example.com";
    let private = created_private_domain(&gateway, private_name, "the private record").await;
    let private_id = private
        .id
        .clone()
        .expect("a private record is created with a domainNameId");
    let regional = created_regional_domain(&gateway, "refusal-regional.example.com").await;
    for (case, domain) in [
        ("the private record", &private),
        ("the regional record", &regional),
    ] {
        stored_mapping(
            &gateway,
            domain,
            case,
            Some("v1"),
            "v1",
            &api,
            Some(stage.as_str()),
        )
        .await;
    }

    // Every pair here resolves to NO stored domain name, however near it is to one that
    // exists: an unknown name, a stored name differing in one label, the private record's
    // name without its id, with an id no record carries, with its own id extended, and the
    // regional record's name carrying an id (it has none). The refusal is the DOMAIN
    // NAME's own missing-resource sentence, because the record a mapping belongs to is
    // what the pair failed to resolve — whatever base path the request names.
    let pairs: Vec<(&str, DomainRef)> = vec![
        (
            "a name no record carries",
            DomainRef {
                name: "absent.example.com".to_string(),
                id: None,
            },
        ),
        (
            "a stored name differing in one label",
            DomainRef {
                name: "refusal-regional.example.org".to_string(),
                id: None,
            },
        ),
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
    ];
    for (case, domain) in &pairs {
        for base_path in ["v1", NONE_BASE_PATH] {
            let answer = delete_mapping(&gateway, domain, base_path).await;
            assert_not_found(
                &answer,
                NO_SUCH_DOMAIN_NAME,
                &format!("{case}, at the base path {base_path}"),
            );
        }
    }

    // A base path the RESOLVED record holds no mapping at names nothing to delete, which
    // is the mapping's own missing-resource sentence: an unmapped path, one differing in
    // case, one that extends a mapped one, a prefix of a mapped one, and the `(none)` base
    // path neither record has a mapping at.
    for (case, domain) in [
        ("the private record", &private),
        ("the regional record", &regional),
    ] {
        for base_path in ["v2", "V1", "v1x", "v", NONE_BASE_PATH] {
            let answer = delete_mapping(&gateway, domain, base_path).await;
            assert_not_found(
                &answer,
                NO_SUCH_BASE_PATH_MAPPING,
                &format!("the base path {base_path} under {case}"),
            );
        }
    }

    // None of the refusals removed anything: each record's one mapping still reads exactly
    // as its create stored it.
    for (case, domain) in [
        ("the private record", &private),
        ("the regional record", &regional),
    ] {
        let read = get_mapping(&gateway, domain, "v1").await;
        assert_eq!(
            mapping_of(&read.body, case),
            ("v1".to_string(), api.clone(), Some(stage.clone())),
            "case {case} after the refusals: expected the stored mapping, got {} with {:?}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn a_deleted_mapping_frees_its_place_in_the_per_domain_name_quota() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "quota-delete-api", "prod").await;
    let domain = created_regional_domain(&gateway, "quota-delete.example.com").await;

    // Fill the record to API Gateway's published quota of base path mappings.
    for index in 0..MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME {
        let base_path = format!("path-{index}");
        stored_mapping(
            &gateway,
            &domain,
            &format!("create {index} of {MAX_BASE_PATH_MAPPINGS_PER_DOMAIN_NAME}"),
            Some(&base_path),
            &base_path,
            &api,
            Some(stage.as_str()),
        )
        .await;
    }
    let one_past = create_mapping(
        &gateway,
        &domain,
        json!({ "basePath": "one-past", "restApiId": api, "stage": stage }),
    )
    .await;
    assert_failure(&one_past, 429, LIMIT_EXCEEDED, "one past the quota");

    // A delete does not merely hide a record: it removes it, so the record it freed leaves
    // room for exactly one more mapping, and the next create past the quota is refused
    // again.
    assert_deleted(
        &delete_mapping(&gateway, &domain, "path-0").await,
        "the first mapping at the quota",
    );
    stored_mapping(
        &gateway,
        &domain,
        "the create the delete made room for",
        Some("one-past"),
        "one-past",
        &api,
        Some(stage.as_str()),
    )
    .await;
    let refused_again = create_mapping(
        &gateway,
        &domain,
        json!({ "basePath": "two-past", "restApiId": api, "stage": stage }),
    )
    .await;
    assert_failure(
        &refused_again,
        429,
        LIMIT_EXCEEDED,
        "one past the quota again",
    );
    // The deleted base path is gone and the one the delete made room for reads.
    assert_not_found(
        &get_mapping(&gateway, &domain, "path-0").await,
        NO_SUCH_BASE_PATH_MAPPING,
        "the deleted mapping",
    );
    let read = get_mapping(&gateway, &domain, "one-past").await;
    assert_eq!(
        mapping_of(&read.body, "the create the delete made room for"),
        ("one-past".to_string(), api.clone(), Some(stage.clone())),
        "case the create the delete made room for: expected it to read, got {} with {:?}",
        read.status,
        read.body
    );
}
