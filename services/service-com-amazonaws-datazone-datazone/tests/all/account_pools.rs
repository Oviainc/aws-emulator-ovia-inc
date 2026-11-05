// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone account pools.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateAccountPool` mints a pool under a persisted, available domain and stores
//!    the whole record, so `GetAccountPool` reports exactly what was stored — both
//!    `AccountSource` members (a static account list and a custom handler) included.
//! 2. The values the service itself supplies are derived, not echoed: the id is minted
//!    and unique, `createdBy` comes from the caller's signature, and `domainUnitId` is
//!    the domain's own root unit.
//! 3. A pool is scoped to its domain and disappears with it.
//! 4. The documented prerequisite is enforced against the store: the domain must exist
//!    and be available, and a pool nothing created is not found.
//! 5. A request outside a modelled constraint is refused with the modelled validation
//!    error, not stored.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};
// The one `ResolutionStrategy` member as the service stores it, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::RESOLUTION_STRATEGY_MANUAL;

/// `AccountPoolId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="AccountPoolId").
fn is_account_pool_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One `CreateAccountPool` request body.
struct PoolCase {
    label: &'static str,
    body: Value,
}

/// The cases span what the rule generalises over: both members of the `AccountSource`
/// union, the size of a static account list (1, 3 and the modelled maximum of 25), the
/// number of regions per account, an account name and a description present or absent,
/// and the name's size and character class.
///
/// SOURCE for every bound and class named below:
/// `smithy_query(query="shape", shape="AccountSource,AccountInfo,AccountPoolName,AwsRegionList,CustomAccountPoolHandler")`.
fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            label: "one account, one region, the shortest name, no description",
            body: json!({
                // `AccountPoolName` is 1..64 of `^[\w -]+$`; this is the minimum length.
                "name": "p",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        // `AwsAccountId` is `^\d{12}$`, `AwsRegionList` is 1..3 of
                        // `^[a-z]{2}-[a-z]{4,10}-\d$`.
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            }),
        },
        PoolCase {
            label: "three accounts, account names, up to three regions, a description",
            body: json!({
                "name": "Finance pool-1",
                "description": "the accounts finance deploys into",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        {
                            "awsAccountId": "222233334444",
                            "supportedRegions": ["us-west-2"],
                            "awsAccountName": "finance-prod",
                        },
                        {
                            "awsAccountId": "333344445555",
                            "supportedRegions": ["eu-central-1", "ap-southeast-2"],
                        },
                        {
                            "awsAccountId": "444455556666",
                            "supportedRegions": ["us-east-1", "us-east-2", "eu-west-1"],
                            "awsAccountName": "finance-sandbox",
                        },
                    ]
                },
            }),
        },
        PoolCase {
            label: "the modelled maximum of 25 accounts and a 64-character name",
            body: json!({
                "name": "n".repeat(64),
                // `Description` is ..2048; this is the maximum.
                "description": "d".repeat(2048),
                "resolutionStrategy": "MANUAL",
                "accountSource": { "accounts": accounts(25) },
            }),
        },
        PoolCase {
            label: "a custom Lambda handler with an execution role",
            body: json!({
                "name": "custom_handler pool",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "customAccountPoolHandler": {
                        "lambdaFunctionArn":
                            "arn:aws:lambda:us-west-2:111122223333:function:resolve-accounts",
                        "lambdaExecutionRoleArn":
                            "arn:aws:iam::111122223333:role/service-role/AccountPoolHandler",
                    }
                },
            }),
        },
        PoolCase {
            label: "a custom Lambda handler with the function ARN only",
            body: json!({
                "name": "handler only",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "customAccountPoolHandler": {
                        "lambdaFunctionArn":
                            "arn:aws:lambda:eu-west-1:999988887777:function:pick_accounts:PROD",
                    }
                },
            }),
        },
    ]
}

/// `count` distinct accounts, each with one supported region: the account ids are
/// twelve digits (`AwsAccountId`) built from the index.
fn accounts(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "awsAccountId": format!("{:012}", 100_000_000_000u64 + index as u64),
                "supportedRegions": ["us-east-1"],
            })
        })
        .collect()
}

/// An available domain plus the `rootDomainUnitId` it reports: a new pool must belong
/// to that unit, so the test needs both.
async fn available_domain_with_root(dz: &Emulator, name: &str) -> (String, String) {
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": name })))
        .await;
    assert_eq!(
        created.status, 201,
        "creating domain {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    // Wait out the chosen lifecycle span, plus slack, so the domain has settled to
    // AVAILABLE and can take an account pool.
    tokio::time::sleep(SETTLED).await;
    (created.s("id"), created.s("rootDomainUnitId"))
}

/// The members a pool's create and get answers must agree on with the request.
fn assert_matches_request(label: &str, answer_body: &Value, request: &Value, source: &str) {
    for member in ["name", "description"] {
        assert_eq!(
            answer_body.get(member).and_then(Value::as_str),
            request.get(member).and_then(Value::as_str),
            "case {label:?}: {source} must report the persisted {member}, got {:?}",
            answer_body.get(member)
        );
    }
    assert_eq!(
        answer_body.get("accountSource"),
        request.get("accountSource"),
        "case {label:?}: {source} must report the persisted accountSource, got {:?}",
        answer_body.get("accountSource")
    );
    assert_eq!(
        answer_body.get("resolutionStrategy").and_then(Value::as_str),
        Some(RESOLUTION_STRATEGY_MANUAL),
        "case {label:?}: {source} must report the stored resolutionStrategy, got {:?}",
        answer_body.get("resolutionStrategy")
    );
}

#[tokio::test]
async fn created_account_pools_are_persisted_scoped_and_reported_by_get() {
    let dz = Emulator::new();
    let (domain, root_unit) = available_domain_with_root(&dz, "account-pools-domain").await;
    let (other_domain, _) = available_domain_with_root(&dz, "account-pools-elsewhere").await;
    let pools_uri = format!("/v2/domains/{domain}/account-pools");

    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for case in pool_cases() {
        let answer = dz.call("POST", &pools_uri, Some(case.body.clone())).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_account_pool_id(&id),
            "case {:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        assert_eq!(
            answer.s("domainId"),
            domain,
            "case {:?}: expected the pool to report its domain, got {:?}",
            case.label,
            answer.s("domainId")
        );
        assert_matches_request(case.label, &answer.body, &case.body, "CreateAccountPool");
        // The pool belongs to the domain's ROOT unit: the request names no domain unit.
        assert_eq!(
            answer.s("domainUnitId"),
            root_unit,
            "case {:?}: expected the domain's root unit as domainUnitId, got {:?}",
            case.label,
            answer.s("domainUnitId")
        );
        // `createdBy` is the CALLER's principal, derived from the request's SigV4
        // credential scope — never an echo of the request body.
        assert_eq!(
            answer.s("createdBy"),
            "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY",
            "case {:?}: expected the signing principal as createdBy, got {:?}",
            case.label,
            answer.s("createdBy")
        );
        assert!(
            answer.body.get("createdAt").is_some(),
            "case {:?}: expected createdAt, got {:?}",
            case.label,
            answer.body
        );
        // Nothing has updated the pool, so no principal is reported as its updater.
        assert!(
            answer.body.get("updatedBy").is_none(),
            "case {:?}: a pool nothing updated must report no updatedBy, got {:?}",
            case.label,
            answer.body
        );
        created.push((case.label, id, case.body));
    }

    // Every id is distinct: two pools never share one.
    let mut ids: Vec<&String> = created.iter().map(|(_, id, _)| id).collect();
    ids.sort();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "each created account pool must have its own id, got {ids:?}"
    );

    // GetAccountPool reports exactly what CreateAccountPool stored.
    for (label, id, request) in &created {
        let got = dz.call("GET", &format!("{pools_uri}/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetAccountPool, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.s("id"),
            *id,
            "case {label:?}: GetAccountPool must report the pool that was read, got {:?}",
            got.s("id")
        );
        assert_eq!(
            got.s("domainId"),
            domain,
            "case {label:?}: GetAccountPool must report the stored domain, got {:?}",
            got.s("domainId")
        );
        assert_matches_request(label, &got.body, request, "GetAccountPool");
        assert_eq!(
            got.s("domainUnitId"),
            root_unit,
            "case {label:?}: GetAccountPool must report the stored domain unit, got {:?}",
            got.s("domainUnitId")
        );
        assert_eq!(
            got.s("createdBy"),
            "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY",
            "case {label:?}: GetAccountPool must report the stored creator, got {:?}",
            got.s("createdBy")
        );
        assert!(
            got.body.get("createdAt").is_some(),
            "case {label:?}: expected createdAt from GetAccountPool, got {:?}",
            got.body
        );
    }

    // A pool is scoped to its domain: the SAME id read through another domain is as
    // absent as one that was never created.
    for (label, id, _) in &created {
        let cross = dz
            .call(
                "GET",
                &format!("/v2/domains/{other_domain}/account-pools/{id}"),
                None,
            )
            .await;
        assert_eq!(
            cross.status, 404,
            "case {label:?}: a pool of another domain must not be readable here, got {} {:?}",
            cross.status, cross.body
        );
        assert_eq!(
            cross.error_code, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            cross.error_code
        );
    }

    // The same request bodies are accepted in another domain, as that domain's own
    // pools — a pool's namespace is its domain.
    for (label, _, request) in &created {
        let elsewhere = dz
            .call(
                "POST",
                &format!("/v2/domains/{other_domain}/account-pools"),
                Some(request.clone()),
            )
            .await;
        assert_eq!(
            elsewhere.status, 201,
            "case {label:?}: the same pool in another domain must be accepted, got {} {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.s("domainId"),
            other_domain,
            "case {label:?}: expected the other domain, got {:?}",
            elsewhere.s("domainId")
        );
    }

    // Deleting the domain takes its account pools with it.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    for (label, id, _) in &created {
        let gone = dz.call("GET", &format!("{pools_uri}/{id}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {label:?}: a pool must go with its domain, got {} {:?}",
            gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn an_account_pool_needs_a_domain_that_exists_and_is_available() {
    let dz = Emulator::new();
    let (domain, _) = available_domain_with_root(&dz, "pool-prerequisites").await;

    // A well-formed domain id nothing created: a permission is held ON a domain, so the
    // caller holds none here and authorization decides the create before any lookup.
    for case in pool_cases() {
        let answer = dz
            .call(
                "POST",
                "/v2/domains/dzd_nosuchdomain000/account-pools",
                Some(case.body),
            )
            .await;
        assert_eq!(
            answer.status, 403,
            "case {:?}: expected 403 for a domain nothing created, got {} {:?}",
            case.label, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "AccessDeniedException",
            "case {:?}: expected AccessDeniedException, got {:?}",
            case.label, answer.error_code
        );
        assert_eq!(
            answer.s("message"),
            "User is not permitted to perform operation: CreateAccountPool",
            "case {:?}: the denial must carry the recorded wording, got {:?}",
            case.label,
            answer.body
        );
    }

    // A domain that is still being created cannot take a pool yet: it is reported in a
    // non-terminal status, so the request conflicts with that status.
    let creating = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "pool-domain-still-creating" })),
        )
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the domain: expected 201, got {} {:?}",
        creating.status, creating.body
    );
    let too_early = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/account-pools", creating.s("id")),
            Some(pool_cases().swap_remove(0).body),
        )
        .await;
    assert_eq!(
        too_early.status, 409,
        "a domain that has not settled yet must refuse a pool, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code, "ConflictException",
        "expected ConflictException, got {:?}",
        too_early.error_code
    );

    // A pool id nothing created is not found, in a domain that does exist.
    for id in ["nosuchpool", "0", &"z".repeat(36)] {
        let missing = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/account-pools/{id}"),
                None,
            )
            .await;
        assert_eq!(
            missing.status, 404,
            "reading pool {id:?}: expected 404, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_code, "ResourceNotFoundException",
            "reading pool {id:?}: expected ResourceNotFoundException, got {:?}",
            missing.error_code
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let dz = Emulator::new();
    let (domain, _) = available_domain_with_root(&dz, "pool-constraints").await;
    let pools_uri = format!("/v2/domains/{domain}/account-pools");

    // Each row is one modelled constraint, named with the shape it comes from, taken
    // one step PAST its published bound or outside its class.
    let refused: Vec<(&str, Value)> = vec![
        (
            // `AccountInfoList` is 1..25.
            "an empty account list",
            json!({
                "name": "empty",
                "resolutionStrategy": "MANUAL",
                "accountSource": { "accounts": [] },
            }),
        ),
        (
            "26 accounts, one past the modelled maximum",
            json!({
                "name": "too many",
                "resolutionStrategy": "MANUAL",
                "accountSource": { "accounts": accounts(26) },
            }),
        ),
        (
            // `AwsAccountId` is `^\d{12}$`.
            "an account id of eleven digits",
            json!({
                "name": "short account id",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "11112222333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            }),
        ),
        (
            // `AwsRegionList` is 1..3.
            "four supported regions, one past the modelled maximum",
            json!({
                "name": "too many regions",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [{
                        "awsAccountId": "111122223333",
                        "supportedRegions": ["us-east-1", "us-east-2", "us-west-1", "us-west-2"],
                    }]
                },
            }),
        ),
        (
            // `AwsRegion` is `^[a-z]{2}-[a-z]{4,10}-\d$`.
            "a region outside the modelled class",
            json!({
                "name": "bad region",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["US-EAST-1"] }
                    ]
                },
            }),
        ),
        (
            // `AccountPoolName` is 1..64 of `^[\w -]+$`.
            "a 65-character name, one past the modelled maximum",
            json!({
                "name": "n".repeat(65),
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            }),
        ),
        (
            "a name outside the modelled character class",
            json!({
                "name": "pool!",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            }),
        ),
        (
            // `ResolutionStrategy`'s only member is MANUAL.
            "a resolution strategy the enum does not have",
            json!({
                "name": "bad strategy",
                "resolutionStrategy": "AUTOMATIC",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            }),
        ),
    ];

    for (label, body) in refused {
        let answer = dz.call("POST", &pools_uri, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }
}
