// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the records a `BasePathMapping` holds a REFERENCE to, and what
//! that reference refuses: `DeleteDomainName` and `DeleteStage`, read back through
//! `GetDomainName`, `GetStage` and `GetBasePathMapping`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `CreateDomainName` / `CreateBasePathMapping`
//! helpers are this crate's existing harness.
//!
//! The RULE under test: a base path mapping routes the calls arriving on its base path of
//! a custom domain name to a stage of a REST API, so while it stands none of the records
//! it names may be deleted — the domain name that HOLDS the mapping, the REST API it names
//! and the stage of that API it names are each refused with the modelled
//! `BadRequestException`, naming the mapping, and every record survives. Deleting the
//! mapping releases them all. Nothing else is held: a record no mapping points at deletes
//! as it always did.
//!
//! The rows differ along what the rule generalises over: the spelling of the base path
//! (a plain one and the `(none)` the member documents), whether the mapping names a stage
//! at all, how many mappings the domain name holds, and which of several stages and APIs
//! the mapping points at.

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    domain_name_has_base_path_mappings, rest_api_in_use_by_base_path_mapping,
    stage_in_use_by_base_path_mapping, BasePathMappingReference, NONE_BASE_PATH,
};

use crate::base_path_mapping::{create_mapping, deployed_api};
use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};

const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// `DeleteDomainName` for `domain_name`.
async fn delete_domain(gateway: &Gateway, domain_name: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/domainnames/{domain_name}"), None)
        .await
}

/// `GetDomainName` for `domain_name`.
async fn get_domain(gateway: &Gateway, domain_name: &str) -> Answer {
    gateway
        .call("GET", &format!("/domainnames/{domain_name}"), None)
        .await
}

/// `DeleteStage` for `stage_name` of `api`.
async fn delete_stage(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/stages/{stage_name}"),
            None,
        )
        .await
}

/// `DeleteRestApi` for `api`.
async fn delete_api(gateway: &Gateway, api: &str) -> Answer {
    gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await
}

/// `DeleteBasePathMapping` for `base_path` of `domain_name`.
async fn delete_mapping(gateway: &Gateway, domain_name: &str, base_path: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/domainnames/{domain_name}/basepathmappings/{base_path}"),
            None,
        )
        .await
}

/// Assert `answer` is the modelled `BadRequestException` carrying exactly `message`.
fn assert_bad_request(answer: &Answer, message: &str, case: &str) {
    assert_failure(answer, BAD_REQUEST.0, BAD_REQUEST.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// One row: the case, the base path the mapping is created under, and whether the mapping
/// names the stage.
struct MappingRow {
    case: &'static str,
    base_path: &'static str,
    with_stage: bool,
}

#[tokio::test]
async fn a_domain_name_that_still_holds_a_base_path_mapping_cannot_be_deleted() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "held-domain-api", "prod").await;

    let rows = [
        MappingRow {
            case: "a plain base path",
            base_path: "v1",
            with_stage: true,
        },
        MappingRow {
            case: "the (none) base path",
            base_path: NONE_BASE_PATH,
            with_stage: true,
        },
        MappingRow {
            case: "a mapping that names no stage",
            base_path: "stageless",
            with_stage: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let domain_name = format!("held-{index}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;

        // Before the mapping exists the domain name is deletable — the guard is the
        // mapping, not the domain name.
        let mut request = json!({ "basePath": row.base_path, "restApiId": api });
        if row.with_stage {
            request["stage"] = json!(stage);
        }
        let created = create_mapping(&gateway, &domain_name, request).await;
        assert_eq!(
            created.status, 201,
            "case {}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            row.case, created.status, created.body
        );

        let refused = delete_domain(&gateway, &domain_name).await;
        assert_bad_request(
            &refused,
            &domain_name_has_base_path_mappings(&domain_name, row.base_path),
            row.case,
        );

        // The record survives the refusal, with the mapping it holds.
        let read = get_domain(&gateway, &domain_name).await;
        assert_eq!(
            read.status, 200,
            "case {}: the refused delete must leave the domain name, got {} with {:?}",
            row.case, read.status, read.body
        );

        // Releasing the mapping releases the domain name: the delete that was refused now
        // succeeds, and the record stops being reported.
        let released = delete_mapping(&gateway, &domain_name, row.base_path).await;
        assert_eq!(
            released.status, 202,
            "case {}: expected 202 from DeleteBasePathMapping, got {} with {:?}",
            row.case, released.status, released.body
        );
        let deleted = delete_domain(&gateway, &domain_name).await;
        assert_eq!(
            deleted.status, 202,
            "case {}: expected 202 from DeleteDomainName once the mapping is gone, got {} \
             with {:?}",
            row.case, deleted.status, deleted.body
        );
        let gone = get_domain(&gateway, &domain_name).await;
        assert_eq!(
            gone.status, 404,
            "case {}: expected the deleted domain name to be gone, got {} with {:?}",
            row.case, gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn every_mapping_of_a_domain_name_has_to_go_before_the_domain_name_does() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "many-mappings-api", "prod").await;
    let domain_name = "many-mappings.example.com";
    created_domain(&gateway, domain_name, json!({})).await;

    let base_paths = ["alpha", "beta", "gamma"];
    for base_path in base_paths {
        create_mapping(
            &gateway,
            domain_name,
            json!({ "basePath": base_path, "restApiId": api, "stage": stage }),
        )
        .await;
    }

    // The refusal names the FIRST mapping created, so a caller walking the refusals is
    // told one call at a time which mapping to remove, and the domain name goes only when
    // the last of them has.
    for (index, base_path) in base_paths.iter().enumerate() {
        let case = format!("{} mapping(s) left", base_paths.len() - index);
        let refused = delete_domain(&gateway, domain_name).await;
        assert_bad_request(
            &refused,
            &domain_name_has_base_path_mappings(domain_name, base_path),
            &case,
        );
        let released = delete_mapping(&gateway, domain_name, base_path).await;
        assert_eq!(
            released.status, 202,
            "case {case}: expected 202 from DeleteBasePathMapping, got {} with {:?}",
            released.status, released.body
        );
    }

    let deleted = delete_domain(&gateway, domain_name).await;
    assert_eq!(
        deleted.status, 202,
        "case no mappings left: expected 202 from DeleteDomainName, got {} with {:?}",
        deleted.status, deleted.body
    );
}

#[tokio::test]
async fn a_stage_a_base_path_mapping_still_names_cannot_be_deleted() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "held-stage-api", "prod").await;
    // A second stage of the same API and a second API with a same-named stage: neither is
    // the stage the mapping names, so neither is held.
    create_deployed_stage(&gateway, &api, "beta").await;
    let (other_api, other_stage) = deployed_api(&gateway, "free-stage-api", "prod").await;

    let rows = [
        MappingRow {
            case: "a plain base path",
            base_path: "v1",
            with_stage: true,
        },
        MappingRow {
            case: "the (none) base path",
            base_path: NONE_BASE_PATH,
            with_stage: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let domain_name = format!("held-stage-{index}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;
        create_mapping(
            &gateway,
            &domain_name,
            json!({ "basePath": row.base_path, "restApiId": api, "stage": stage }),
        )
        .await;

        let referencing = BasePathMappingReference {
            domain_name: domain_name.clone(),
            base_path: row.base_path.to_string(),
        };
        let refused = delete_stage(&gateway, &api, &stage).await;
        assert_bad_request(
            &refused,
            &stage_in_use_by_base_path_mapping(&stage, &referencing),
            row.case,
        );

        // The stage survives the refusal.
        let read = get_stage(&gateway, &api, &stage).await;
        assert_eq!(
            read.status, 200,
            "case {}: the refused delete must leave the stage, got {} with {:?}",
            row.case, read.status, read.body
        );

        // Nothing else is held: the API's other stage, and the same-named stage of
        // another API, are deletable while this mapping stands — and are restored for the
        // next row.
        for (free_api, free_stage) in [(&api, "beta"), (&other_api, other_stage.as_str())] {
            let deleted = delete_stage(&gateway, free_api, free_stage).await;
            assert_eq!(
                deleted.status, 202,
                "case {}: a stage no mapping names must delete, got {} with {:?}",
                row.case, deleted.status, deleted.body
            );
            create_deployed_stage(&gateway, free_api, free_stage).await;
        }

        // Releasing the mapping releases the stage.
        delete_mapping(&gateway, &domain_name, row.base_path).await;
        let deleted = delete_stage(&gateway, &api, &stage).await;
        assert_eq!(
            deleted.status, 202,
            "case {}: expected 202 from DeleteStage once the mapping is gone, got {} with {:?}",
            row.case, deleted.status, deleted.body
        );
        let gone = get_stage(&gateway, &api, &stage).await;
        assert_eq!(
            gone.status, 404,
            "case {}: expected the deleted stage to be gone, got {} with {:?}",
            row.case, gone.status, gone.body
        );
        // Restored for the next row, which needs the same stage back.
        create_deployed_stage(&gateway, &api, &stage).await;
    }
}

#[tokio::test]
async fn a_rest_api_a_base_path_mapping_still_names_cannot_be_deleted() {
    let gateway = Gateway::new();
    // The rows differ along what the rule generalises over: whether the mapping names a
    // stage of the API at all, and the spelling of its base path. A mapping names its API
    // either way, so either way the API is held.
    let rows = [
        MappingRow {
            case: "a mapping that names a stage",
            base_path: "v1",
            with_stage: true,
        },
        MappingRow {
            case: "a mapping that names no stage",
            base_path: "stageless",
            with_stage: false,
        },
        MappingRow {
            case: "a mapping at the (none) base path",
            base_path: NONE_BASE_PATH,
            with_stage: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (api, stage) = deployed_api(&gateway, &format!("held-api-{index}"), "prod").await;
        // An API no mapping names deletes, which is what makes the refusal below the
        // mapping's doing and not the API's.
        let (free_api, _) = deployed_api(&gateway, &format!("free-api-{index}"), "prod").await;
        let deleted = delete_api(&gateway, &free_api).await;
        assert_eq!(
            deleted.status, 202,
            "case {}: an API no mapping names must delete, got {} with {:?}",
            row.case, deleted.status, deleted.body
        );

        let domain_name = format!("held-api-{index}.example.com");
        created_domain(&gateway, &domain_name, json!({})).await;
        let mut request = json!({ "basePath": row.base_path, "restApiId": api });
        if row.with_stage {
            request["stage"] = json!(stage);
        }
        create_mapping(&gateway, &domain_name, request).await;

        let referencing = BasePathMappingReference {
            domain_name: domain_name.clone(),
            base_path: row.base_path.to_string(),
        };
        let refused = delete_api(&gateway, &api).await;
        assert_bad_request(
            &refused,
            &rest_api_in_use_by_base_path_mapping(&api, &referencing),
            row.case,
        );

        // The API survives the refusal, with the stage the mapping routes to.
        let read = gateway
            .call("GET", &format!("/restapis/{api}"), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {}: the refused delete must leave the API, got {} with {:?}",
            row.case, read.status, read.body
        );

        // Releasing the mapping releases the API.
        delete_mapping(&gateway, &domain_name, row.base_path).await;
        let deleted = delete_api(&gateway, &api).await;
        assert_eq!(
            deleted.status, 202,
            "case {}: expected 202 from DeleteRestApi once the mapping is gone, got {} with {:?}",
            row.case, deleted.status, deleted.body
        );
        let gone = gateway
            .call("GET", &format!("/restapis/{api}"), None)
            .await;
        assert_eq!(
            gone.status, 404,
            "case {}: expected the deleted API to be gone, got {} with {:?}",
            row.case, gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn a_mapping_that_names_no_stage_holds_no_stage() {
    let gateway = Gateway::new();
    let (api, stage) = deployed_api(&gateway, "stageless-mapping-api", "prod").await;
    let domain_name = "stageless-mapping.example.com";
    created_domain(&gateway, domain_name, json!({})).await;

    // "Specify '(none)' if you want callers to explicitly specify the stage name after any
    // base path name": a mapping that names no stage, and one that names `(none)`, name no
    // stage of the API at all, so neither holds one.
    for (case, request) in [
        (
            "a mapping created without a stage",
            json!({ "basePath": "no-stage", "restApiId": api }),
        ),
        (
            "a mapping whose stage is (none)",
            json!({ "basePath": "none-stage", "restApiId": api, "stage": NONE_BASE_PATH }),
        ),
    ] {
        let created = create_mapping(&gateway, domain_name, request).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
            created.status, created.body
        );

        let deleted = delete_stage(&gateway, &api, &stage).await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: a stage no mapping names must delete, got {} with {:?}",
            deleted.status, deleted.body
        );
        // Restored for the next row.
        create_deployed_stage(&gateway, &api, &stage).await;
    }
}
