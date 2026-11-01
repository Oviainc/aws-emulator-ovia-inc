// SPDX-License-Identifier: CC0-1.0

//! What each entitlement an agreement granted is provisioned TO, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: an agreement is involved with a set of resources — the
//! products of the offer it was created from, or the ones the agreement it
//! derives from was involved with — it reports that same set in
//! `ProposalSummary$resources`, and accepting it provisions ONE entitlement per
//! resource. Each entitlement names the resource it was provisioned to (its id
//! and its product type), the type of entitlement it is, its own AWS License
//! Manager licence and its own short-lived registration token, and keeps all
//! four while the agreement moves its status. A read reports them: it mints
//! nothing and changes nothing a later caller sees.
//!
//! The harness (`emulator`, `call`, `create`, `accept`, `agreement_over`,
//! `describe`, `search`, `cancel`, `term`) is the one `agreement_lifecycle`
//! owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, cancel, create, describe, emulator,
    search, term, variable_payment_term,
};

/// The products the offer this emulator publishes is for, as `(id, type)` — the
/// resources every agreement created from that offer is involved with.
///
/// Spelled out here rather than imported from the crate, so an acceptance that
/// provisioned some other product, or none, cannot make this suite pass. A
/// `Resource$id` is "also a `productId`", and the types are the product types
/// `Resource$type` documents ("for example, `SaaSProduct`, `AmiProduct`,
/// `ContainerProduct`").
const PUBLISHED_PRODUCTS: &[(&str, &str)] = &[
    ("prod-emulatorsaas", "SaaSProduct"),
    ("prod-emulatorami", "AmiProduct"),
    ("prod-emulatorcontainer", "ContainerProduct"),
];

/// `AgreementEntitlement$type`, "the type of entitlement": every entitlement
/// this service provisions is an AWS License Manager licence
/// (`AgreementEntitlement$licenseArn`), which is the kind each reports.
/// `EntitlementType` is `@length(1, 64)` with `^[A-Za-z:]+$`.
const ENTITLEMENT_TYPE: &str = "License";

/// How many characters a registration token must carry at least, so a token
/// that is a short constant or a guessable stub fails this suite. The modelled
/// ceiling is `RegistrationToken`'s `@length(max: 512)`.
const TOKEN_MIN_LENGTH: usize = 32;

/// Whether `value` is a `RegistrationToken` as the model admits one:
/// `@length(1, 512)` with `^[A-Za-z0-9+/=.:_-]+$`, spelled out from the grammar
/// rather than built with the crate's own minter.
fn is_registration_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '.' | ':' | '_' | '-'))
}

/// The entitlements of one agreement, asserting the call succeeded.
async fn entitlements_of(router: &MultiServiceRouter, agreement_id: &str) -> Vec<Value> {
    let answer = call(
        router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `(id, type)` pairs a `Resource` list carries, in the order it carries
/// them.
fn resource_pairs(resources: &Value) -> Vec<(String, String)> {
    resources
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|resource| {
            (
                resource["id"].as_str().unwrap_or_default().to_string(),
                resource["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The resource each entitlement in `granted` was provisioned to, in the order
/// they were provisioned.
fn provisioned_to(granted: &[Value]) -> Vec<(String, String)> {
    granted
        .iter()
        .map(|entitlement| {
            (
                entitlement["resource"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                entitlement["resource"]["type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The published products as the pairs the reports above are compared against.
fn published_pairs() -> Vec<(String, String)> {
    PUBLISHED_PRODUCTS
        .iter()
        .map(|(id, product_type)| ((*id).to_string(), (*product_type).to_string()))
        .collect()
}

/// Create and accept an agreement derived from `source` under `intent`,
/// answering its agreement id.
async fn derived_agreement(
    router: &MultiServiceRouter,
    intent: &str,
    proposal: Option<&str>,
    source: &str,
    terms: Vec<Value>,
) -> String {
    let mut request = json!({
        "intent": intent,
        "sourceAgreementIdentifier": source,
        "requestedTerms": terms,
    });
    if let Some(proposal) = proposal {
        request["agreementProposalIdentifier"] = json!(proposal);
    }
    let created = create(router, request).await;
    assert_eq!(
        created.status, 200,
        "creating the {intent} request must succeed, got {} with {:?}",
        created.status, created.body
    );
    let accepted = accept(
        router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the {intent} request must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    accepted.str_at("/agreementId")
}

/// Accepting an agreement request provisions one entitlement per resource the
/// agreement is involved with, each naming that resource and the type of
/// entitlement it is, and the agreement reports the SAME resources in its
/// proposal summary — so a client can resolve an entitlement to the product it
/// entitles.
///
/// The rows differ along what the rule generalises over: how the agreement came
/// to be (a new one, a replacement), how many terms it was accepted over and of
/// which kinds. None of those changes the resources it is involved with, which
/// is what the entitlement count follows. An amendment grants nothing of its
/// own, which the tail of this test asserts: it modifies an existing agreement,
/// whose entitlements already entitle its acceptor to those resources.
#[tokio::test]
async fn an_agreement_grants_one_entitlement_per_resource_it_is_involved_with() {
    let router = emulator();

    let plain = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;
    let many_terms = agreement_over(
        &router,
        "ap-offer2",
        vec![
            term("LegalTerm"),
            term("SupportTerm"),
            variable_payment_term("vpt-1"),
        ],
    )
    .await;
    let amended = derived_agreement(&router, "AMEND", None, &plain, vec![term("eula.v2")]).await;
    let replacement = derived_agreement(
        &router,
        "REPLACE",
        Some("ap-offer3"),
        &many_terms,
        vec![term("eula.v2")],
    )
    .await;

    let cases: Vec<(&str, &String)> = vec![
        ("a new agreement over one term", &plain),
        ("a new agreement over three terms", &many_terms),
        ("a replacement of an agreement", &replacement),
    ];

    let mut licenses: BTreeSet<String> = BTreeSet::new();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for (case, agreement) in cases {
        let granted = entitlements_of(&router, agreement).await;
        assert_eq!(
            granted.len(),
            PUBLISHED_PRODUCTS.len(),
            "case {case:?}: one entitlement per resource the agreement is involved with, \
             got {granted:?}"
        );
        assert_eq!(
            provisioned_to(&granted),
            published_pairs(),
            "case {case:?}: each entitlement must name the resource it was provisioned \
             to, got {granted:?}"
        );

        for entitlement in &granted {
            let resource = entitlement["resource"].clone();
            assert_eq!(
                entitlement["type"].as_str(),
                Some(ENTITLEMENT_TYPE),
                "case {case:?}: the entitlement of {resource:?} must report its type, got \
                 {entitlement:?}"
            );
            let license = entitlement["licenseArn"].as_str().unwrap_or_default();
            assert!(
                licenses.insert(license.to_string()),
                "case {case:?}: every entitlement carries its own licence, but {license} \
                 was already granted ({entitlement:?})"
            );
            let token = entitlement["registrationToken"].as_str().unwrap_or_default();
            assert!(
                is_registration_token(token) && token.len() >= TOKEN_MIN_LENGTH,
                "case {case:?}: the entitlement of {resource:?} must carry a registration \
                 token of at least {TOKEN_MIN_LENGTH} characters, got {entitlement:?}"
            );
            assert!(
                tokens.insert(token.to_string()),
                "case {case:?}: every entitlement carries its own token, but the one of \
                 {resource:?} was already minted"
            );
        }

        // The resources the entitlements were provisioned to are the resources
        // the agreement itself reports: one list, read back through the
        // agreement's own reader.
        let described = describe(&router, agreement).await;
        assert_eq!(
            resource_pairs(&described.body["proposalSummary"]["resources"]),
            provisioned_to(&granted),
            "case {case:?}: the agreement must report the resources its entitlements were \
             provisioned to, got {:?}",
            described.body
        );
    }

    // An amendment "modifies an existing agreement with terms that are accepted
    // in the request", so it answers that agreement and grants no second
    // licence for resources it already entitles its acceptor to: the licences
    // the amended agreement reports are the ones it was already reporting, and
    // none of them is a licence this test has not already seen.
    let case = "an amendment of an agreement";
    assert_eq!(
        &amended, &plain,
        "case {case:?}: the accept must answer the agreement it modified, got {amended:?} \
         against {plain:?}"
    );
    let after = entitlements_of(&router, &amended).await;
    let granted_licenses: Vec<String> = after
        .iter()
        .map(|entitlement| {
            entitlement["licenseArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        granted_licenses.len(),
        PUBLISHED_PRODUCTS.len(),
        "case {case:?}: the amended agreement still grants one entitlement per resource, \
         got {after:?}"
    );
    for license in &granted_licenses {
        assert!(
            licenses.contains(license),
            "case {case:?}: an amendment grants no new licence, got {license} in {after:?}"
        );
    }
}

/// `GetAgreementEntitlements` reports what provisioning stored: the resource,
/// the type, the licence and the registration token of an entitlement are the
/// same on a second read, and they are the same once the agreement that granted
/// them is no longer active — a status moves, an entitlement's identity does
/// not.
#[tokio::test]
async fn an_entitlement_keeps_its_resource_type_and_token_across_reads_and_statuses() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    /// The members of an entitlement that are its identity rather than its
    /// state, as a reader sees them.
    fn identity(granted: &[Value]) -> Vec<Value> {
        granted
            .iter()
            .map(|entitlement| {
                json!({
                    "resource": entitlement["resource"],
                    "type": entitlement["type"],
                    "licenseArn": entitlement["licenseArn"],
                    "registrationToken": entitlement["registrationToken"],
                })
            })
            .collect()
    }

    let first = entitlements_of(&router, &agreement).await;
    let before = identity(&first);
    assert_eq!(
        before.len(),
        PUBLISHED_PRODUCTS.len(),
        "the agreement must grant one entitlement per published product, got {first:?}"
    );

    let again = entitlements_of(&router, &agreement).await;
    assert_eq!(
        identity(&again),
        before,
        "case \"a second read\": a read reports and mints nothing, so the resource, type, \
         licence and token must not change, got {again:?}"
    );

    let cancellation = cancel(&router, &agreement).await;
    assert_eq!(
        cancellation.status, 200,
        "cancelling {agreement} must succeed, got {} with {:?}",
        cancellation.status, cancellation.body
    );

    let after_cancellation = entitlements_of(&router, &agreement).await;
    assert_eq!(
        identity(&after_cancellation),
        before,
        "case \"a cancelled agreement\": deprovisioning moves the status, not the resource, \
         type, licence or token, got {after_cancellation:?}"
    );
    for entitlement in &after_cancellation {
        assert_eq!(
            entitlement["status"].as_str(),
            Some("DEPROVISIONED"),
            "case \"a cancelled agreement\": the entitlement of {:?} is no longer \
             provisioned, got {entitlement:?}",
            entitlement["resource"]
        );
    }
}

/// An agreement is searchable by each resource it is involved with: the
/// `ResourceIdentifier` filter selects it by product id and the `ResourceType`
/// filter by product type, while an id or a type no published product carries
/// selects nothing.
///
/// The rows are every published product plus one of each kind that nothing
/// publishes, so a filter that matched everything, or nothing, fails here.
#[tokio::test]
async fn a_search_selects_an_agreement_by_the_resources_it_is_involved_with() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    /// The agreement ids one filter selects.
    async fn selected(router: &MultiServiceRouter, name: &str, value: &str) -> Vec<String> {
        let answer = search(
            router,
            json!({
                "catalog": "AWSMarketplace",
                "filters": [
                    agreement_type_filter(),
                    {"name": name, "values": [value]},
                ],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}={value:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        answer.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["agreementId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    let mut cases: Vec<(&str, String, Vec<String>)> = Vec::new();
    for (id, product_type) in PUBLISHED_PRODUCTS {
        cases.push((
            "ResourceIdentifier",
            (*id).to_string(),
            vec![agreement.clone()],
        ));
        cases.push((
            "ResourceType",
            (*product_type).to_string(),
            vec![agreement.clone()],
        ));
    }
    // A product id and a product type no published product carries: a filter on
    // something no agreement is involved with selects nothing.
    cases.push((
        "ResourceIdentifier",
        "prod-nothingpublishesthis".to_string(),
        vec![],
    ));
    cases.push(("ResourceType", "MachineLearningProduct".to_string(), vec![]));

    for (name, value, expected) in cases {
        let selected = selected(&router, name, &value).await;
        assert_eq!(
            selected, expected,
            "case {name:?}={value:?}: expected {expected:?}, got {selected:?}"
        );
    }
}
