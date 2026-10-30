// SPDX-License-Identifier: CC0-1.0

//! The account-entitlement gate of Security Lake's data-lake control plane, end to end
//! over HTTP.
//!
//! RULE under test: Security Lake's data lake is administered by the organization's
//! delegated Security Lake administrator or by a standalone Security Lake account, so a
//! caller account holding neither role may not configure or inspect a data lake. The
//! verdict is reached from the caller's role ALONE — before the request is validated and
//! before anything is looked up or written — and each gated route reports it the way
//! Security Lake reports it there: the routes the service denies itself answer the
//! modelled `AccessDeniedException` (HTTP 403) carrying the fixed entitlement text, and
//! the two whose authorizer refuses ahead of the service answer HTTP 401
//! `UnauthorizedException` / "Unauthorized". `ListDataLakes`, which only reports which of
//! the caller's own Regions are enabled, is not gated.
//!
//! Every case drives a real `restJson1` request through the router into this service, on
//! the shared harness in `data_lake_and_subscriber_lifecycle`; only the account-topology
//! record itself is set through the store, because no modelled operation writes it
//! (`RegisterDataLakeDelegatedAdministrator` is the organization MANAGEMENT account's
//! call, not a call of the account being administered).

use serde_json::{json, Value};
use service_com_amazonaws_securitylake_securitylake::state::AccountRole;

use super::data_lake_and_subscriber_lifecycle::{array, text, Emulator, Reply};

/// The message Security Lake refuses a gated route with for a caller account that holds
/// no Security Lake role. RECORDED text, written out here rather than read from the
/// service, so this case pins the wording: it is entirely FIXED — it interpolates no
/// account, Region or role from the request or from the account's state.
const ENTITLEMENT_MESSAGE: &str = "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account.";

/// A `RoleArn`-shaped value, matching `CreateDataLakeRequest$metaStoreManagerRoleArn`'s
/// pattern `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const META_STORE_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonSecurityLakeMetaStoreManager";

/// The roles that may administer Security Lake, and so lift the gate.
const ADMINISTERING_ROLES: &[(&str, AccountRole)] = &[
    ("the delegated administrator", AccountRole::DelegatedAdministrator),
    ("a standalone account", AccountRole::Standalone),
];

/// How a gated route reports a caller account that holds no Security Lake role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The service denies the call: the modelled `AccessDeniedException` at HTTP 403,
    /// carrying the entitlement text.
    AccessDenied,
    /// The route's authorizer refuses it ahead of the service: HTTP 401
    /// `UnauthorizedException` / "Unauthorized".
    Unauthorized,
}

impl Refusal {
    /// The `(status, error code, message)` triple this refusal is stated as.
    fn expected(self) -> (u16, &'static str, &'static str) {
        match self {
            Self::AccessDenied => (403, "AccessDeniedException", ENTITLEMENT_MESSAGE),
            Self::Unauthorized => (401, "UnauthorizedException", "Unauthorized"),
        }
    }
}

/// One request this suite sends.
struct Call {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
}

/// Every operation the gate covers, one row each, with the refusal its route reports.
///
/// The rows span the whole data-lake control plane — the data lake itself, its exception
/// subscription and its organization configuration — and every HTTP method the service
/// binds to those routes (`POST`, `PUT`, `GET`, `DELETE`), because the verdict comes from
/// the caller's role rather than from the shape of the call.
fn gated_calls() -> Vec<(Call, Refusal)> {
    vec![
        (
            Call {
                case: "CreateDataLake",
                method: "POST",
                path: "/v1/datalake",
                body: Some(json!({
                    "configurations": [{ "region": "us-east-1" }],
                    "metaStoreManagerRoleArn": META_STORE_ROLE,
                })),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "UpdateDataLake",
                method: "PUT",
                path: "/v1/datalake",
                body: Some(json!({
                    "configurations": [{ "region": "us-east-1" }],
                    "metaStoreManagerRoleArn": META_STORE_ROLE,
                })),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "DeleteDataLake",
                method: "POST",
                path: "/v1/datalake/delete",
                body: Some(json!({ "regions": ["us-east-1"] })),
            },
            Refusal::Unauthorized,
        ),
        (
            Call {
                case: "GetDataLakeSources, no accounts member",
                method: "POST",
                path: "/v1/datalake/sources",
                body: Some(json!({})),
            },
            Refusal::Unauthorized,
        ),
        (
            Call {
                case: "GetDataLakeSources, accounts naming the caller",
                method: "POST",
                path: "/v1/datalake/sources",
                body: Some(json!({ "accounts": [::engine::EMULATOR_ACCOUNT_ID] })),
            },
            Refusal::Unauthorized,
        ),
        (
            Call {
                case: "CreateDataLakeExceptionSubscription",
                method: "POST",
                path: "/v1/datalake/exceptions/subscription",
                body: Some(json!({
                    "subscriptionProtocol": "sqs",
                    "notificationEndpoint": "arn:aws:sqs:us-east-1:000000000000:exceptions",
                })),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "GetDataLakeExceptionSubscription",
                method: "GET",
                path: "/v1/datalake/exceptions/subscription",
                body: None,
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "UpdateDataLakeExceptionSubscription",
                method: "PUT",
                path: "/v1/datalake/exceptions/subscription",
                body: Some(json!({
                    "subscriptionProtocol": "https",
                    "notificationEndpoint": "https://example.invalid/exceptions",
                })),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "DeleteDataLakeExceptionSubscription",
                method: "DELETE",
                path: "/v1/datalake/exceptions/subscription",
                body: None,
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "ListDataLakeExceptions",
                method: "POST",
                path: "/v1/datalake/exceptions",
                body: Some(json!({})),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "CreateDataLakeOrganizationConfiguration",
                method: "POST",
                path: "/v1/datalake/organization/configuration",
                body: Some(json!({})),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "GetDataLakeOrganizationConfiguration",
                method: "GET",
                path: "/v1/datalake/organization/configuration",
                body: None,
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "DeleteDataLakeOrganizationConfiguration",
                method: "POST",
                path: "/v1/datalake/organization/configuration/delete",
                body: Some(json!({})),
            },
            Refusal::AccessDenied,
        ),
    ]
}

/// An emulator whose caller account holds no Security Lake role — the state an account
/// starts in.
fn unentitled() -> Emulator {
    let emulator = Emulator::new();
    emulator.set_account_role(AccountRole::OrganizationMember);
    emulator
}

/// The `message` an error reply carries, or `None` when it carries none.
fn message(reply: &Reply) -> Option<&str> {
    reply.body["message"].as_str()
}

/// Assert `reply` is exactly how `refusal` is stated.
fn assert_refused(case: &str, reply: &Reply, refusal: Refusal) {
    let (status, code, text) = refusal.expected();
    assert_eq!(
        (reply.status, reply.error_type.as_deref(), message(reply)),
        (status, Some(code), Some(text)),
        "case {case:?}: expected {status} {code} with the refusal message, got {} {:?} with {:?}",
        reply.status,
        reply.error_type,
        reply.body
    );
}

/// RULE: with the caller account holding no Security Lake role, every gated call is
/// refused the way its route reports that refusal — and nothing it would have written is
/// applied, which the ungated `ListDataLakes` reports by still listing no data lake.
#[tokio::test]
async fn every_gated_call_is_refused_for_an_account_with_no_security_lake_role() {
    // Rows span the calling Region too: the verdict is about the ACCOUNT, so no Region
    // may answer differently.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-2"] {
        for (call, refusal) in gated_calls() {
            let emulator = unentitled();
            let case = format!("{} in {region}", call.case);

            let reply = emulator
                .send(call.method, call.path, region, call.body.clone())
                .await;
            assert_refused(&case, &reply, refusal);

            // The positive control, and the proof the refused call half-applied nothing:
            // ListDataLakes is not gated, and it still reports an account with no data
            // lake.
            let listed = emulator.list_data_lakes(region, "").await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListDataLakes is not gated, got {} with {:?}",
                listed.status, listed.body
            );
            assert!(
                array(&listed, "dataLakes", &case).is_empty(),
                "case {case:?}: a refused call must persist nothing, got {:?}",
                listed.body
            );
        }
    }
}

/// RULE: the entitlement verdict precedes request-level validation, so a gated call is
/// refused identically however badly formed it is — the caller never learns which of its
/// members the service would have rejected.
#[tokio::test]
async fn the_refusal_precedes_request_validation() {
    const REGION: &str = "us-west-2";

    // Rows: each kind of request the boundary validator would otherwise refuse — an
    // absent required member, a value outside a declared pattern, and one outside a
    // declared range — on gated routes of both refusal kinds.
    let rows: Vec<(Call, Refusal)> = vec![
        (
            Call {
                case: "CreateDataLake with neither required member",
                method: "POST",
                path: "/v1/datalake",
                body: Some(json!({})),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "CreateDataLake with a metaStoreManagerRoleArn outside its pattern",
                method: "POST",
                path: "/v1/datalake",
                body: Some(json!({
                    "configurations": [{ "region": REGION }],
                    "metaStoreManagerRoleArn": "not-an-iam-role-arn",
                })),
            },
            Refusal::AccessDenied,
        ),
        (
            Call {
                case: "DeleteDataLake with no regions member",
                method: "POST",
                path: "/v1/datalake/delete",
                body: Some(json!({})),
            },
            Refusal::Unauthorized,
        ),
        (
            Call {
                case: "GetDataLakeSources with maxResults past its range",
                method: "POST",
                path: "/v1/datalake/sources",
                body: Some(json!({ "maxResults": 101 })),
            },
            Refusal::Unauthorized,
        ),
    ];

    for (call, refusal) in rows {
        let emulator = unentitled();
        let reply = emulator
            .send(call.method, call.path, REGION, call.body.clone())
            .await;
        assert_refused(call.case, &reply, refusal);
    }
}

/// RULE: the refusal is reached from the role alone, so every route the service denies
/// itself answers the IDENTICAL body — no account id, Region, role or member name from
/// the request reaches it.
#[tokio::test]
async fn the_denial_body_is_identical_on_every_denied_route() {
    const REGION: &str = "eu-central-1";

    let mut answers: Vec<(String, Value)> = Vec::new();
    for (call, refusal) in gated_calls() {
        if refusal != Refusal::AccessDenied {
            continue;
        }
        let emulator = unentitled();
        let reply = emulator
            .send(call.method, call.path, REGION, call.body.clone())
            .await;
        assert_refused(call.case, &reply, refusal);
        answers.push((call.case.to_string(), reply.body));
    }

    let (first_case, first) = answers
        .first()
        .cloned()
        .expect("the table holds denied routes");
    for (case, body) in &answers[1..] {
        assert_eq!(
            body, &first,
            "case {case:?}: the denial must be identical to {first_case:?}, got {body:?} \
             against {first:?}"
        );
    }
}

/// RULE: an account that DOES administer Security Lake — the delegated administrator or a
/// standalone account — is served; and taking the role away closes the gate again without
/// touching what the account already holds.
#[tokio::test]
async fn an_administering_account_is_served_and_losing_the_role_closes_the_gate() {
    const REGION: &str = "us-east-2";

    for (case, role) in ADMINISTERING_ROLES {
        let emulator = unentitled();
        emulator.set_account_role(*role);

        let created = emulator
            .create_data_lake(REGION, json!({ "region": REGION }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateDataLake must be served, got {} with {:?}",
            created.status, created.body
        );

        // A gated route whose handler is still a 501 scaffold: what changed for an
        // administering account is that the gate no longer answers, so the refusal must
        // not be the entitlement one.
        let subscription = emulator
            .send(
                "GET",
                "/v1/datalake/exceptions/subscription",
                REGION,
                None,
            )
            .await;
        assert_ne!(
            (subscription.status, message(&subscription)),
            (403, Some(ENTITLEMENT_MESSAGE)),
            "case {case:?}: an administering account must get past the gate, got {:?}",
            subscription.body
        );

        // Take the role away: the same call is refused again, and the data lake created
        // while the account held it is still there for an administering caller.
        emulator.set_account_role(AccountRole::OrganizationMember);
        let refused = emulator
            .create_data_lake(REGION, json!({ "region": REGION }))
            .await;
        assert_refused(case, &refused, Refusal::AccessDenied);

        emulator.set_account_role(*role);
        let listed = emulator.list_data_lakes(REGION, "").await;
        let regions: Vec<String> = array(&listed, "dataLakes", case)
            .iter()
            .map(|lake| text(lake, "region", case).to_string())
            .collect();
        assert_eq!(
            regions,
            vec![REGION.to_string()],
            "case {case:?}: the stored data lake must survive the refusals, got {:?}",
            listed.body
        );
    }
}

// ---------------------------------------------------------------------------
// GetDataLakeSources, once the caller may ask
// ---------------------------------------------------------------------------

/// Send `GetDataLakeSources` with `body` as the Region `region`.
async fn get_sources(emulator: &Emulator, region: &str, body: Value) -> Reply {
    emulator
        .send("POST", "/v1/datalake/sources", region, Some(body))
        .await
}

/// RULE: the snapshot is of ONE Region — the one the request is signed for — so it exists
/// only while that Region holds a data lake, and reports the data lake that is stored
/// there rather than anything echoed from the request.
#[tokio::test]
async fn get_data_lake_sources_reports_the_region_it_is_called_in() {
    // Rows: the Region the data lake is created in, and the Region the read is signed
    // for. Only the matching rows hold a snapshot.
    let rows: [(&str, &str, &str); 4] = [
        ("enabled and read in the same Region", "us-east-1", "us-east-1"),
        ("enabled and read in another same Region", "ap-south-1", "ap-south-1"),
        ("enabled elsewhere, read in us-east-1", "eu-west-1", "us-east-1"),
        ("enabled elsewhere, read in ap-south-1", "us-east-1", "ap-south-1"),
    ];

    for (case, enabled_region, reading_region) in rows {
        let emulator = Emulator::new();
        let created = emulator
            .create_data_lake(enabled_region, json!({ "region": enabled_region }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: enabling {enabled_region:?} got {} with {:?}",
            created.status, created.body
        );
        let stored_arn = text(&array(&created, "dataLakes", case)[0], "dataLakeArn", case).to_string();

        let reply = get_sources(&emulator, reading_region, json!({})).await;

        if enabled_region == reading_region {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the snapshot, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.body["dataLakeArn"].as_str(),
                Some(stored_arn.as_str()),
                "case {case:?}: the snapshot must report the stored data lake's ARN, got {:?}",
                reply.body
            );
            let sources = array(&reply, "dataLakeSources", case);
            let accounts: Vec<String> = sources
                .iter()
                .map(|source| text(source, "account", case).to_string())
                .collect();
            assert_eq!(
                accounts,
                vec![::engine::emulator_account_id().to_string()],
                "case {case:?}: the snapshot covers the account Security Lake is enabled \
                 for, got {:?}",
                reply.body
            );
            assert!(
                reply.body["nextToken"].is_null(),
                "case {case:?}: one entry needs no continuation token, got {:?}",
                reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a Region with no data lake has no snapshot, got {} with {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                message(&reply),
                Some(format!("Security Lake is not enabled in Region {reading_region}").as_str()),
                "case {case:?}: wrong refusal message, got {:?}",
                reply.body
            );
            assert_eq!(
                (
                    reply.body["resourceName"].as_str(),
                    reply.body["resourceType"].as_str()
                ),
                (Some(reading_region), Some("DataLake")),
                "case {case:?}: the refusal must name the missing data lake, got {:?}",
                reply.body
            );
        }
    }
}

/// RULE: `accounts` narrows the snapshot to the accounts it names; an account Security
/// Lake is not enabled for is never reported, and the snapshot's `dataLakeArn` is
/// reported either way.
#[tokio::test]
async fn get_data_lake_sources_narrows_the_snapshot_to_the_accounts_named() {
    const REGION: &str = "eu-west-3";
    let caller = ::engine::emulator_account_id().to_string();

    // Rows: the `accounts` filter, and the accounts the snapshot must then cover. The
    // filter is `uniqueItems` over 12-digit account ids (`AccountList`).
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no accounts member", json!({}), vec![caller.clone()]),
        (
            "the caller's account",
            json!({ "accounts": [caller.clone()] }),
            vec![caller.clone()],
        ),
        (
            "the caller's account beside another",
            json!({ "accounts": [caller.clone(), "210987654321"] }),
            vec![caller.clone()],
        ),
        (
            "one other account",
            json!({ "accounts": ["210987654321"] }),
            Vec::new(),
        ),
        (
            "two other accounts",
            json!({ "accounts": ["210987654321", "345678901234"] }),
            Vec::new(),
        ),
    ];

    let emulator = Emulator::new();
    let created = emulator
        .create_data_lake(REGION, json!({ "region": REGION }))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: enabling {REGION:?} got {} with {:?}",
        created.status, created.body
    );

    for (case, body, expected) in rows {
        let reply = get_sources(&emulator, REGION, body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the snapshot, got {} with {:?}",
            reply.status, reply.body
        );
        let accounts: Vec<String> = array(&reply, "dataLakeSources", case)
            .iter()
            .map(|source| text(source, "account", case).to_string())
            .collect();
        assert_eq!(
            accounts, expected,
            "case {case:?}: wrong accounts covered, got {:?}",
            reply.body
        );
        assert!(
            reply.body["dataLakeArn"].as_str().is_some(),
            "case {case:?}: the snapshot names its data lake whatever it covers, got {:?}",
            reply.body
        );
    }
}

/// RULE: the read is paginated — `maxResults` bounds the page, a token this service did
/// not issue is refused as a bad request, and a token resumes the page order rather than
/// restarting it.
#[tokio::test]
async fn get_data_lake_sources_pages_the_snapshot() {
    const REGION: &str = "ca-central-1";
    let caller = ::engine::emulator_account_id().to_string();

    let emulator = Emulator::new();
    let created = emulator
        .create_data_lake(REGION, json!({ "region": REGION }))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: enabling {REGION:?} got {} with {:?}",
        created.status, created.body
    );

    // Rows: `maxResults` at each bound of the model's `@range(min: 1, max: 100)` and at
    // its `@default`. One account is enabled, so every page holds that one entry and
    // none needs a continuation token.
    for max_results in [1, 50, 100] {
        let case = format!("maxResults={max_results}");
        let reply = get_sources(&emulator, REGION, json!({ "maxResults": max_results })).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the snapshot, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            array(&reply, "dataLakeSources", &case).len(),
            1,
            "case {case:?}: expected the one covered account, got {:?}",
            reply.body
        );
        assert!(
            reply.body["nextToken"].is_null(),
            "case {case:?}: nothing follows the page, so no token, got {:?}",
            reply.body
        );
    }

    // Rows: tokens this service never issued, in the forms a client can send — not hex
    // at all, hex of text with no index, and hex of an index with no anchor.
    for token in [
        "not-hex".to_string(),
        hex::encode("no-index"),
        hex::encode("7:"),
    ] {
        let case = format!("nextToken={token:?}");
        let reply = get_sources(&emulator, REGION, json!({ "nextToken": token })).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a token this service did not issue is a bad request, got {} \
             with {:?}",
            reply.status,
            reply.body
        );
    }

    // A token anchored on the one covered account resumes at it; a token whose anchor is
    // no longer covered falls back to the position it recorded, which is past the end
    // here, so the page is empty.
    let resumed = get_sources(
        &emulator,
        REGION,
        json!({ "nextToken": hex::encode(format!("0:{caller}")) }),
    )
    .await;
    assert_eq!(
        array(&resumed, "dataLakeSources", "a token anchored on the covered account").len(),
        1,
        "case \"a token anchored on the covered account\": expected the entry, got {:?}",
        resumed.body
    );
    let past_the_end = get_sources(
        &emulator,
        REGION,
        json!({ "nextToken": hex::encode("4:210987654321") }),
    )
    .await;
    assert!(
        array(&past_the_end, "dataLakeSources", "a token past the end").is_empty(),
        "case \"a token past the end\": expected an empty page, got {:?}",
        past_the_end.body
    );
    assert!(
        past_the_end.body["nextToken"].is_null(),
        "case \"a token past the end\": an empty page needs no token, got {:?}",
        past_the_end.body
    );
}

/// RULE: the snapshot tracks the stored data lake, so deleting it takes the snapshot with
/// it — the read goes back to the modelled 404.
#[tokio::test]
async fn deleting_the_data_lake_takes_the_snapshot_with_it() {
    const REGION: &str = "us-west-1";

    let emulator = Emulator::new();
    let created = emulator
        .create_data_lake(REGION, json!({ "region": REGION }))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: enabling {REGION:?} got {} with {:?}",
        created.status, created.body
    );
    let before = get_sources(&emulator, REGION, json!({})).await;
    assert_eq!(
        before.status, 200,
        "case \"before the delete\": expected the snapshot, got {} with {:?}",
        before.status, before.body
    );

    let deleted = emulator.delete_data_lake(REGION, &[REGION]).await;
    assert_eq!(
        deleted.status, 200,
        "case \"the delete\": got {} with {:?}",
        deleted.status, deleted.body
    );

    let after = get_sources(&emulator, REGION, json!({})).await;
    assert_eq!(
        (after.status, after.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"after the delete\": the snapshot must be gone, got {} with {:?}",
        after.status,
        after.body
    );
}
