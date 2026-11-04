// SPDX-License-Identifier: CC0-1.0

//! AWS Billing Conductor's payer-account gate, over the WHOLE router.
//!
//! RULE under test: Billing Conductor is a payer-account-only API, so every operation of
//! its control plane refuses a caller that is not the payer (management) account of the
//! consolidated billing family with HTTP 403 `AccessDeniedException` — the shape the model
//! lists as a common error of every operation of this service — carried in the
//! `x-amzn-errortype` header restJson1 resolves an error shape from, with a `Message`
//! built FROM THE REQUEST (the calling account id and the operation name), and that
//! refusal comes before any state is read or written; the same calls made by the payer
//! account the store records are served from persisted state.
//!
//! The service's own suites (`tests/all/payer_account_gate.rs` and its siblings) pin the
//! gate over a router holding billingconductor ALONE. This suite is about the production
//! path: the assembled multi-service router, where another service's claim or signing name
//! could take these routes, must answer the same way — the denial for a non-payer and the
//! create → read round-trip for the payer, on the one engine every service shares.
//!
//! The rows span the kind of operation (a create, a paginated read, an association, a
//! delete) and which account the store records as the payer (none, another account, this
//! one).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use service_com_amazonaws_billingconductor_awsbillingconductor::ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState;
use service_com_amazonaws_organizations_awsorganizationsv20161128::state::{
    AccountRecord as OrganizationsAccountRecord, ChildRef,
    ComAmazonawsOrganizationsAwsorganizationsv20161128State as OrganizationsState,
    ACCOUNT_JOINED_METHOD_CREATED, ACCOUNT_STATUS_ACTIVE, ARN_REGION, ARN_SERVICE,
    CHILD_TYPE_ACCOUNT,
};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// An AWS-managed pricing plan the `PricingPlanFullArn` pattern enumerates, so a billing
/// group can name one without creating a plan first.
const MANAGED_PLAN_ARN: &str = "arn:aws:billingconductor::aws:pricingplan/Passthrough";

/// A 12-digit account that is not the one this emulator's callers authenticate as.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// The account the payer groups below: a linked account of the consolidated billing
/// family, which it becomes by being a member account of the organization
/// ([`link_account_to_the_organization`]).
const LINKED_ACCOUNT: &str = "000000000001";

/// Put `account_id` in the consolidated billing family the way an account joins one: as a
/// member account of the organization the Organizations store records.
///
/// Billing Conductor adds no account to a family — it READS the family's roster from the
/// Organizations store on the shared engine — so this is where a linked account of the
/// emulated account's family comes from. The record is the one that store keeps for a
/// member account, including the root's child edge, so the roster this links into is the
/// same one `ListAccounts` would report.
fn link_account_to_the_organization(engine: &InMemoryEngine, account_id: &str) {
    let state = engine.state::<OrganizationsState>();
    let mut store = state.lock();
    let organization_id = store
        .organization
        .as_ref()
        .expect("the emulated account belongs to an organization")
        .id
        .clone();
    let root_id = store
        .root()
        .expect("the organization has a root")
        .id
        .clone();
    store.children.entry(root_id).or_default().push(ChildRef {
        id: account_id.to_string(),
        child_type: CHILD_TYPE_ACCOUNT.to_string(),
    });
    store.accounts.insert(
        account_id.to_string(),
        OrganizationsAccountRecord {
            arn: engine::emulator_arn(
                ARN_SERVICE,
                ARN_REGION,
                &format!("account/{organization_id}/{account_id}"),
            ),
            id: account_id.to_string(),
            email: format!("{account_id}@example.com"),
            name: format!("Linked account {account_id}"),
            status: ACCOUNT_STATUS_ACTIVE.to_string(),
            joined_method: ACCOUNT_JOINED_METHOD_CREATED.to_string(),
            joined_timestamp: aws_smithy_types::DateTime::from(std::time::SystemTime::now()),
        },
    );
}

/// A SigV4 `Authorization` header whose credential scope names this service. The
/// signature is never checked: this router runs with authentication off.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/billingconductor/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The answer to one request: status, the `x-amzn-errortype` header and the parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: serde_json::Value,
}

/// Drive one billingconductor request through the assembled router (`restJson1`: the
/// operation is the path).
fn billingconductor(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    path: &str,
    body: &serde_json::Value,
) -> Reply {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://billingconductor.{REGION}.amazonaws.com{path}"
        ))
        .header("host", format!("billingconductor.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// The restJson1 error code, without its namespace.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The `Message` the gate owes, read from the SERVICE's own builder
/// (`state::not_payer_account`) rather than re-spelled here, so this suite cannot drift
/// from the text the service puts on the wire.
fn expected_message(operation: &str) -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::not_payer_account(operation)
        .message
}

/// One call per kind of operation: `(case, operation, path, body)`. None of the resources
/// these bodies name exists, so a denial that leaked a lookup would show up as a 404 or a
/// `ValidationException` instead of the 403.
fn every_kind_of_operation() -> Vec<(&'static str, &'static str, &'static str, serde_json::Value)>
{
    let absent_group = format!(
        "arn:aws:billingconductor::{}:billinggroup/ABCDEFGHIJ",
        engine::emulator_account_id()
    );
    vec![
        (
            "a paginated read",
            "ListBillingGroups",
            "/list-billing-groups",
            serde_json::json!({"MaxResults": 10}),
        ),
        (
            "a list over another collection",
            "ListAccountAssociations",
            "/list-account-associations",
            serde_json::json!({}),
        ),
        (
            "a create",
            "CreatePricingPlan",
            "/create-pricing-plan",
            serde_json::json!({"Name": "PayerOnlyPlan"}),
        ),
        (
            "an association",
            "AssociateAccounts",
            "/associate-accounts",
            serde_json::json!({"Arn": absent_group, "AccountIds": [LINKED_ACCOUNT]}),
        ),
        (
            "a delete",
            "DeleteBillingGroup",
            "/delete-billing-group",
            serde_json::json!({"Arn": absent_group}),
        ),
    ]
}

#[test]
fn billing_conductor_gates_the_payer_account_over_the_assembled_router() {
    std::thread::Builder::new()
        .name("billingconductor-payer-account".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(Arc::clone(&engine));
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let designate_payer = |payer: Option<&str>| {
                engine
                    .state::<BillingConductorState>()
                    .lock()
                    .payer_account_id = payer.map(str::to_string);
            };

            // The two worlds in which the caller is NOT the payer account: the store as it
            // stands up (its payer slot holds no account) and a store whose payer is some
            // other account.
            for (world, payer) in [
                ("the store records no payer account", None),
                ("another account is the payer account", Some(ANOTHER_ACCOUNT)),
            ] {
                designate_payer(payer);
                for (case, operation, path, body) in every_kind_of_operation() {
                    let refused = billingconductor(&router, &runtime, path, &body);
                    assert_eq!(
                        refused.status, 403,
                        "case {case:?} ({operation}) with {world}: expected HTTP 403, got {} {}",
                        refused.status, refused.body
                    );
                    assert_eq!(
                        refused.error_type.as_deref(),
                        Some("AccessDeniedException"),
                        "case {case:?} ({operation}) with {world}: expected \
                         x-amzn-errortype: AccessDeniedException, got {:?}",
                        refused.error_type
                    );
                    assert_eq!(
                        error_code(&refused.body),
                        "AccessDeniedException",
                        "case {case:?} ({operation}) with {world}: expected the modelled \
                         AccessDeniedException, got {}",
                        refused.body
                    );
                    let message = refused
                        .body
                        .get("Message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    assert_eq!(
                        message,
                        expected_message(operation),
                        "case {case:?} ({operation}) with {world}: expected the \
                         request-derived refusal Message, got {}",
                        refused.body
                    );
                    // And that text IS derived from the request, not a constant: it names
                    // the calling account and the operation being called.
                    assert!(
                        message.contains(engine::emulator_account_id())
                            && message.contains(operation),
                        "case {case:?} ({operation}) with {world}: expected the Message to \
                         name the caller {} and the operation, got {message:?}",
                        engine::emulator_account_id()
                    );
                }
            }

            // The third world: this account IS the payer account, so the control plane is
            // served — and what it stores is read back through the paginated list. The
            // account it groups is a linked account of the family, which is an
            // Organizations fact this service only reads.
            designate_payer(Some(engine::emulator_account_id()));
            link_account_to_the_organization(&engine, LINKED_ACCOUNT);
            let group = billingconductor(
                &router,
                &runtime,
                "/create-billing-group",
                &serde_json::json!({
                    "Name": "PayerGroup",
                    "AccountGrouping": {"LinkedAccountIds": [LINKED_ACCOUNT]},
                    "ComputationPreference": {"PricingPlanArn": MANAGED_PLAN_ARN},
                    "PrimaryAccountId": LINKED_ACCOUNT,
                }),
            );
            assert_eq!(
                group.status, 200,
                "CreateBillingGroup must be served to the payer account, got {} {}",
                group.status, group.body
            );
            let group_arn = group.body["Arn"]
                .as_str()
                .expect("the created group carries an ARN")
                .to_string();

            let listed = billingconductor(
                &router,
                &runtime,
                "/list-billing-groups",
                &serde_json::json!({}),
            );
            assert_eq!(
                listed.status, 200,
                "ListBillingGroups must be served to the payer account, got {} {}",
                listed.status, listed.body
            );
            let groups = listed.body["BillingGroups"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                groups.len(),
                1,
                "the stored group must be reported back, got {}",
                listed.body
            );
            assert_eq!(
                groups[0]["Arn"].as_str(),
                Some(group_arn.as_str()),
                "the reported group must be the one that was created, got {}",
                listed.body
            );

            // And the refusals above wrote nothing: the one group the payer created is all
            // the store holds, so no denied create or delete took effect.
            designate_payer(Some(ANOTHER_ACCOUNT));
            let refused_again = billingconductor(
                &router,
                &runtime,
                "/delete-billing-group",
                &serde_json::json!({"Arn": group_arn}),
            );
            assert_eq!(
                refused_again.status, 403,
                "a non-payer delete of an EXISTING group must still be refused, got {} {}",
                refused_again.status, refused_again.body
            );

            designate_payer(Some(engine::emulator_account_id()));
            let after = billingconductor(
                &router,
                &runtime,
                "/list-billing-groups",
                &serde_json::json!({}),
            );
            assert_eq!(
                after.body["BillingGroups"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                1,
                "the refused delete must leave the stored group in place, got {}",
                after.body
            );
        })
        .expect("spawn the payer-account thread")
        .join()
        .expect("billingconductor must gate the payer account over the assembled router");
}
