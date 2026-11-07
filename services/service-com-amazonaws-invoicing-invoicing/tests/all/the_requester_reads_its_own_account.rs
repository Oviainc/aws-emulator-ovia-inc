// SPDX-License-Identifier: CC0-1.0

//! What `BatchGetInvoiceProfile` answers for the REQUESTER'S OWN account, driven end to
//! end over this service's wire protocol (`POST /` with
//! `X-Amz-Target: Invoicing.BatchGetInvoiceProfile` and an awsJson1_0 body, through the
//! same `MultiServiceRouter` the production router uses).
//!
//! The rule under test: a requester may be answered for its own account — it
//! authenticates as that account, so asking about it is asking about its own resources —
//! and for every account this service's state names as a LINKED account of its
//! organization; every other well-formed account id refuses the request WHOLE with the
//! modelled 403 `AccessDeniedException`, whose message names the requester and the account
//! it was refused.
//!
//! The rows span what the rule generalises over: how much state exists around the own
//! account (none at all, one unit naming others, several units naming others, a unit that
//! was deleted again), the shape of the batch it is asked about in (alone, repeated,
//! first, last, among others, at the modelled list maximum) and which other account ids
//! travel beside it (readable ones and refused ones).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The modelled maximum of `AccountIdList` (`@length(1..1000)`,
/// `smithy_query(query="operation", operation="BatchGetInvoiceProfile")`), so the rule is
/// asserted at the largest batch a request may carry.
const MAX_ACCOUNT_IDS: usize = 1000;

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The explanatory text of a failure body, under either case of the message key.
fn error_text(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The requester's own account: the identity this emulator presents to a caller, read from
/// the one source of truth for it rather than spelled as a literal.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// An account id of the modelled `AccountIdString` form (`@pattern ^\d{12}$`), derived from
/// a row index. The offset keeps every generated id distinct from the requester's own
/// account, which is all zeroes.
fn account(index: usize) -> String {
    format!("{:012}", 300 + index)
}

/// The message this service states for an account the requester may not be answered for:
/// built from the request — the requester's own account and the account it was refused.
fn not_authorized_message(refused: &str) -> String {
    format!(
        "Account {} is not authorized to access resources for account {refused}",
        own_account()
    )
}

/// `BatchGetInvoiceProfile` for `accounts`.
async fn batch_get(router: &MultiServiceRouter, accounts: &[String]) -> (u16, Value) {
    call(
        router,
        "BatchGetInvoiceProfile",
        &json!({"AccountIds": accounts}),
    )
    .await
}

/// The `AccountId` of each reported profile, in the order they were reported.
fn reported_accounts(body: &Value) -> Vec<String> {
    body.get("Profiles")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Profiles list in {body}"))
        .iter()
        .map(|profile| {
            profile
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected an AccountId in {profile}"))
                .to_string()
        })
        .collect()
}

/// Create one invoice unit naming `receiver` and `linked`, returning its ARN.
async fn create_unit(
    router: &MultiServiceRouter,
    name: &str,
    receiver: &str,
    linked: &[String],
) -> String {
    let created = ok(
        router,
        "CreateInvoiceUnit",
        &json!({
            "Name": name,
            "InvoiceReceiver": receiver,
            "TaxInheritanceDisabled": false,
            "Rule": {"LinkedAccounts": linked},
            "ClientToken": format!("own-account-{name}"),
        }),
    )
    .await;
    created
        .get("InvoiceUnitArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an InvoiceUnitArn in {created}"))
        .to_string()
}

/// How much state exists around the requester's own account when it asks about itself —
/// the rows of the table below. None of these arrangements names that account, so each one
/// shows the answer does not come from state naming it.
#[derive(Debug, Clone, Copy)]
enum Arrangement {
    /// A fresh emulator: no invoice unit at all, which is how a client first meets this
    /// operation.
    NothingAtAll,
    /// One invoice unit, naming other accounts only.
    OneUnitNamingOthers,
    /// Several invoice units, naming other accounts only.
    SeveralUnitsNamingOthers,
    /// An invoice unit naming other accounts, which was deleted again — so state holds a
    /// retired unit and nothing live.
    OnlyADeletedUnit,
}

/// Apply `arrangement`, returning the OTHER account ids it leaves readable.
async fn arrange(router: &MultiServiceRouter, arrangement: Arrangement) -> Vec<String> {
    match arrangement {
        Arrangement::NothingAtAll => Vec::new(),
        Arrangement::OneUnitNamingOthers => {
            create_unit(router, "one-unit", &account(1), &[account(2)]).await;
            vec![account(1), account(2)]
        }
        Arrangement::SeveralUnitsNamingOthers => {
            create_unit(router, "first-unit", &account(1), &[account(2)]).await;
            create_unit(router, "second-unit", &account(3), &[account(4), account(5)]).await;
            vec![
                account(1),
                account(2),
                account(3),
                account(4),
                account(5),
            ]
        }
        Arrangement::OnlyADeletedUnit => {
            let arn = create_unit(router, "retired-unit", &account(1), &[account(2)]).await;
            ok(
                router,
                "DeleteInvoiceUnit",
                &json!({"InvoiceUnitArn": arn, "ClientToken": "retired-unit-delete"}),
            )
            .await;
            Vec::new()
        }
    }
}

/// RULE: the requester's own account resolves to its invoice profile whatever this
/// service's state holds and whatever shape the batch takes, while an account id the
/// organization does not contain refuses the whole request with the modelled 403 naming it.
#[tokio::test]
async fn the_requesters_own_account_resolves_to_its_profile_whatever_state_holds() {
    let own = own_account();

    for arrangement in [
        Arrangement::NothingAtAll,
        Arrangement::OneUnitNamingOthers,
        Arrangement::SeveralUnitsNamingOthers,
        Arrangement::OnlyADeletedUnit,
    ] {
        let router = emulator();
        let readable = arrange(&router, arrangement).await;

        // The batch shapes the own account can be asked about in, each with the profiles
        // the answer must carry: one per account asked about, in the order the request
        // first names it.
        let mut shapes: Vec<(String, Vec<String>, Vec<String>)> = vec![
            (
                "alone".to_string(),
                vec![own.clone()],
                vec![own.clone()],
            ),
            (
                // `AccountIdList` carries no `@uniqueItems`, so a repeat is admitted.
                "repeated".to_string(),
                vec![own.clone(), own.clone(), own.clone()],
                vec![own.clone()],
            ),
            (
                "at the modelled list maximum".to_string(),
                vec![own.clone(); MAX_ACCOUNT_IDS],
                vec![own.clone()],
            ),
        ];
        if !readable.is_empty() {
            let mut first = vec![own.clone()];
            first.extend(readable.iter().cloned());
            shapes.push(("first of a batch".to_string(), first.clone(), first));
            let mut last = readable.clone();
            last.push(own.clone());
            shapes.push(("last of a batch".to_string(), last.clone(), last));
            let mut between = vec![readable[0].clone(), own.clone()];
            between.extend(readable.iter().skip(1).cloned());
            shapes.push((
                "between linked accounts".to_string(),
                between.clone(),
                between,
            ));
        }

        for (shape, requested, expected) in shapes {
            let label = format!("{arrangement:?}, {shape}");
            let (status, body) = batch_get(&router, &requested).await;
            assert_eq!(
                status, 200,
                "case {label}: expected HTTP 200 for {} account ids, got {status} {body}",
                requested.len()
            );
            let actual = reported_accounts(&body);
            assert_eq!(
                actual, expected,
                "case {label}: expected profiles for {expected:?}, got {actual:?}"
            );
            // The profile this service holds reports the account it is about and invents
            // nothing else: every other member of `InvoiceProfile` is written through a
            // plane this service does not own.
            let profiles = body
                .get("Profiles")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {label}: expected a Profiles list in {body}"));
            for profile in profiles {
                assert_eq!(
                    profile.as_object().map(|members| members.len()),
                    Some(1),
                    "case {label}: a profile reports the account it is about alone, got \
                     {profile}"
                );
            }
        }

        // An id the organization does not contain is refused WHOLE, wherever it sits
        // beside the own account, and the refusal names the requester and that id.
        let outsiders = [account(90), account(91), account(92)];
        for (shape, requested, refused) in [
            (
                "an outsider alone",
                vec![outsiders[0].clone()],
                outsiders[0].clone(),
            ),
            (
                "an outsider after the own account",
                vec![own.clone(), outsiders[1].clone()],
                outsiders[1].clone(),
            ),
            (
                "an outsider before the own account",
                vec![outsiders[2].clone(), own.clone()],
                outsiders[2].clone(),
            ),
            (
                "two outsiders around the own account",
                vec![outsiders[1].clone(), own.clone(), outsiders[2].clone()],
                outsiders[1].clone(),
            ),
        ] {
            let label = format!("{arrangement:?}, {shape}");
            let (status, body) = batch_get(&router, &requested).await;
            assert_eq!(
                status, 403,
                "case {label}: expected HTTP 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {label}: expected the modelled AccessDeniedException, got {body}"
            );
            let expected = not_authorized_message(&refused);
            assert_eq!(
                error_text(&body),
                expected,
                "case {label}: expected the message {expected:?}, got {body}"
            );
            assert!(
                body.get("Profiles").is_none(),
                "case {label}: a refused batch reports no profiles, got {body}"
            );
        }

        // Reading its own profile changes nothing a later caller sees: the same request
        // answers identically, and the invoice units are what they were.
        let units = ok(&router, "ListInvoiceUnits", &json!({})).await;
        let again = batch_get(&router, std::slice::from_ref(&own)).await;
        assert_eq!(
            again.0, 200,
            "case {arrangement:?}, repeated read: expected HTTP 200, got {again:?}"
        );
        assert_eq!(
            units,
            ok(&router, "ListInvoiceUnits", &json!({})).await,
            "case {arrangement:?}: reading a profile must leave the invoice units alone"
        );
    }
}
