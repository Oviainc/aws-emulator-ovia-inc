// SPDX-License-Identifier: CC0-1.0

//! Which refusal answers a write whose `name` is already taken in a policy store
//! that has no room left.
//!
//! The rule under test: every writer of this service that takes a `name`
//! documented as unique within the policy store ("if you specify a name that is
//! already associated with another policy in the policy store, you receive a
//! ConflictException error") decides that duplicate-name conflict BEFORE the
//! store's capacity, so the caller is told which resource already holds the name
//! whatever room the store has left. With a name nothing holds, the very same
//! request on the very same store is still refused for capacity — every row
//! asserts both halves, so the ordering cannot be satisfied by dropping either
//! check.
//!
//! The rows span the writers that take such a name (`CreatePolicy`,
//! `CreatePolicyTemplate`, `UpdatePolicy`, `UpdatePolicyTemplate`) crossed with
//! the capacity dimension that is exhausted: the store's Cedar-text byte budget,
//! and the number of records of that kind the store may hold.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in `tests/all/harness.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions as service;
use smithy_server_runtime::MultiServiceRouter;

use service::state::cedar::MAX_STATEMENT_BYTES;
use service::state::{
    MAX_POLICIES_PER_STORE, MAX_POLICY_BYTES_PER_STORE, MAX_POLICY_TEMPLATES_PER_STORE,
    RESOURCE_TYPE_POLICY, RESOURCE_TYPE_POLICY_TEMPLATE,
};

use crate::harness::{call, error_code, ok, router_with_engine, store_with_validation_off};

/// The scope every static policy this suite writes carries. The store validates
/// nothing (`validationSettings.mode` `OFF`), so these entity types need no
/// schema; the scope is identical across rows because `UpdatePolicy` may not
/// change it.
const POLICY_SCOPE: &str = "permit(principal == PhotoApp::User::\"alice\", \
                            action == PhotoApp::Action::\"viewPhoto\", \
                            resource == PhotoApp::Photo::\"vacation.jpg\")";

/// The same scope as a Cedar TEMPLATE: the principal position is the
/// `?principal` placeholder a template may use. Identical across rows for the
/// same reason — `UpdatePolicyTemplate` may not change a template's scope.
const TEMPLATE_SCOPE: &str = "permit(principal == ?principal, \
                              action == PhotoApp::Action::\"viewPhoto\", \
                              resource == PhotoApp::Photo::\"vacation.jpg\")";

/// The shortest Cedar template this suite writes, for the fill that takes a store
/// to its template COUNT bound while leaving its byte budget untouched.
const SHORT_TEMPLATE: &str = "permit(principal == ?principal, action, resource);";

/// `scope` padded inside a condition clause to exactly `bytes` bytes, so two of
/// these differ only in SIZE. A request for fewer bytes than the scope itself
/// needs yields the unpadded statement, which [`floor_bytes`] reports.
fn padded(scope: &str, bytes: usize) -> String {
    let prefix = format!("{scope} when {{ context.tag == \"");
    let suffix = "\" };";
    let padding = bytes.saturating_sub(prefix.len() + suffix.len());
    format!("{prefix}{}{suffix}", "x".repeat(padding))
}

/// The smallest statement [`padded`] can produce for `scope`.
fn floor_bytes(scope: &str) -> usize {
    padded(scope, 0).len()
}

/// The two resources a policy store holds that carry a name unique within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Policy,
    Template,
}

impl Resource {
    /// The `ResourceType` a conflict with this resource reports.
    fn modelled_type(self) -> &'static str {
        match self {
            Resource::Policy => RESOURCE_TYPE_POLICY,
            Resource::Template => RESOURCE_TYPE_POLICY_TEMPLATE,
        }
    }

    /// A statement of `bytes` bytes for this resource's own Cedar form.
    fn statement(self, bytes: usize) -> String {
        match self {
            Resource::Policy => padded(POLICY_SCOPE, bytes),
            Resource::Template => padded(TEMPLATE_SCOPE, bytes),
        }
    }

    /// The smallest statement this resource's own Cedar form can carry here.
    fn smallest(self) -> String {
        match self {
            Resource::Policy => self.statement(floor_bytes(POLICY_SCOPE)),
            Resource::Template => self.statement(floor_bytes(TEMPLATE_SCOPE)),
        }
    }
}

/// Which capacity dimension of the store a row exhausts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capacity {
    /// The store's total Cedar text, `MAX_POLICY_BYTES_PER_STORE`.
    Bytes,
    /// The number of records of the row's kind the store holds.
    Count,
}

/// Create one resource of `resource` in `store_id` and return its id.
async fn create_one(
    router: &MultiServiceRouter,
    resource: Resource,
    store_id: &str,
    statement: &str,
    name: Option<&str>,
) -> String {
    let (operation, mut request, member) = match resource {
        Resource::Policy => (
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
            "policyId",
        ),
        Resource::Template => (
            "CreatePolicyTemplate",
            json!({ "policyStoreId": store_id, "statement": statement }),
            "policyTemplateId",
        ),
    };
    if let Some(name) = name {
        request["name"] = json!(name);
    }
    let created = ok(router, operation, &request).await;
    created[member]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} must report a minted id, got {created}"))
        .to_string()
}

/// The create under test: a brand-new resource of `resource` carrying `name`,
/// sized at the largest statement one record may hold so it cannot fit in a store
/// that is at its byte budget.
async fn attempt_create(
    router: &MultiServiceRouter,
    resource: Resource,
    store_id: &str,
    name: &str,
) -> (u16, Value) {
    let statement = resource.statement(MAX_STATEMENT_BYTES);
    match resource {
        Resource::Policy => {
            call(
                router,
                "CreatePolicy",
                &json!({
                    "policyStoreId": store_id,
                    "name": name,
                    "definition": { "static": { "statement": statement } },
                }),
            )
            .await
        }
        Resource::Template => {
            call(
                router,
                "CreatePolicyTemplate",
                &json!({
                    "policyStoreId": store_id,
                    "name": name,
                    "statement": statement,
                }),
            )
            .await
        }
    }
}

/// The update under test: `id` renamed to `name`, with a replacement statement at
/// the largest size one record may hold — a replacement a store at its byte
/// budget has no room for, since the bytes the record already holds are far
/// fewer.
async fn attempt_update(
    router: &MultiServiceRouter,
    resource: Resource,
    store_id: &str,
    id: &str,
    name: &str,
) -> (u16, Value) {
    let statement = resource.statement(MAX_STATEMENT_BYTES);
    match resource {
        Resource::Policy => {
            call(
                router,
                "UpdatePolicy",
                &json!({
                    "policyStoreId": store_id,
                    "policyId": id,
                    "name": name,
                    "definition": { "static": { "statement": statement } },
                }),
            )
            .await
        }
        Resource::Template => {
            call(
                router,
                "UpdatePolicyTemplate",
                &json!({
                    "policyStoreId": store_id,
                    "policyTemplateId": id,
                    "name": name,
                    "statement": statement,
                }),
            )
            .await
        }
    }
}

/// The id the `name/<name>` reference resolves to in `store_id`, or `None` when
/// no resource of that kind holds the name.
async fn id_named(
    router: &MultiServiceRouter,
    resource: Resource,
    store_id: &str,
    name: &str,
) -> Option<String> {
    let reference = format!("name/{name}");
    let (operation, request, member) = match resource {
        Resource::Policy => (
            "GetPolicy",
            json!({ "policyStoreId": store_id, "policyId": reference }),
            "policyId",
        ),
        Resource::Template => (
            "GetPolicyTemplate",
            json!({ "policyStoreId": store_id, "policyTemplateId": reference }),
            "policyTemplateId",
        ),
    };
    let (status, body) = call(router, operation, &request).await;
    (status == 200).then(|| body[member].as_str().unwrap_or_default().to_string())
}

/// The Cedar text `id` currently holds, read back through the implemented reader.
async fn stored_statement(
    router: &MultiServiceRouter,
    resource: Resource,
    store_id: &str,
    id: &str,
) -> String {
    match resource {
        Resource::Policy => {
            let body = ok(
                router,
                "GetPolicy",
                &json!({ "policyStoreId": store_id, "policyId": id }),
            )
            .await;
            body["definition"]["static"]["statement"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
        Resource::Template => {
            let body = ok(
                router,
                "GetPolicyTemplate",
                &json!({ "policyStoreId": store_id, "policyTemplateId": id }),
            )
            .await;
            body["statement"].as_str().unwrap_or_default().to_string()
        }
    }
}

/// The bytes of Cedar text `store_id` holds, read straight out of state: the
/// quantity the store's byte budget bounds.
fn cedar_bytes(engine: &Arc<engine::InMemoryEngine>, store_id: &str) -> usize {
    let state = engine.state::<service::ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>();
    let state = state.lock();
    state
        .policy_stores
        .get(store_id)
        .expect("the store this suite created is in state")
        .cedar_bytes()
}

/// Bring `store_id` to EXACTLY `MAX_POLICY_BYTES_PER_STORE` bytes of Cedar text,
/// through `CreatePolicy` — the path a caller fills a store by — in steps of at
/// most the bytes one policy may carry.
async fn fill_to_byte_bound(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    store_id: &str,
) {
    let floor = floor_bytes(POLICY_SCOPE);
    loop {
        let remaining = MAX_POLICY_BYTES_PER_STORE - cedar_bytes(engine, store_id);
        if remaining == 0 {
            break;
        }
        // Each step takes as much as one policy may carry, except where that
        // would leave a tail too small to be a statement at all: then it leaves
        // exactly one smallest statement's worth, so the last step lands on the
        // bound.
        let step = if remaining <= MAX_STATEMENT_BYTES {
            remaining
        } else if remaining - MAX_STATEMENT_BYTES < floor {
            remaining - floor
        } else {
            MAX_STATEMENT_BYTES
        };
        assert!(
            step >= floor,
            "a fill step of {step} bytes is smaller than the {floor}-byte minimum statement"
        );
        let statement = padded(POLICY_SCOPE, step);
        assert_eq!(
            statement.len(),
            step,
            "a fill step must be exactly {step} bytes, got {}",
            statement.len()
        );
        create_one(router, Resource::Policy, store_id, &statement, None).await;
    }
    assert_eq!(
        cedar_bytes(engine, store_id),
        MAX_POLICY_BYTES_PER_STORE,
        "the store should hold exactly its byte budget once filled"
    );
}

/// Bring `store_id` to the per-store COUNT bound for `resource`, seeded straight
/// into state: the published bounds (200,000 policies, 10,000 policy templates)
/// are far more records than a suite can create one request at a time, and what
/// the bound reads is how many records the store holds.
///
/// The records are shaped as the handlers write them, and the seeded policies are
/// TEMPLATE-LINKED — they carry no Cedar text of their own, following
/// `template_id` instead — so the store's byte budget stays well inside its
/// bound. That is what makes these rows the COUNT dimension rather than the byte
/// one; each caller asserts the remaining byte headroom.
fn seed_to_count_bound(
    engine: &Arc<engine::InMemoryEngine>,
    store_id: &str,
    resource: Resource,
    template_id: &str,
) {
    let state = engine.state::<service::ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>();
    let mut state = state.lock();
    let store = state
        .policy_stores
        .get_mut(store_id)
        .expect("the store this suite created is in state");
    match resource {
        Resource::Policy => {
            // The entity the seeded links bind the template's `?principal` to,
            // which is therefore also the principal each linked policy's scope
            // names.
            let bound = service::state::ScopeEntity {
                entity_type: "PhotoApp::User".to_string(),
                entity_id: "alice".to_string(),
            };
            while store.policies.len() < MAX_POLICIES_PER_STORE {
                let policy_id = store.mint_policy_id();
                let created = service::state::now();
                store.policies.insert(
                    policy_id.clone(),
                    service::state::Policy {
                        policy_id,
                        policy_store_id: store_id.to_string(),
                        policy_type: "TEMPLATE_LINKED".to_string(),
                        name: None,
                        statement: None,
                        description: None,
                        template_link: Some(service::state::TemplateLink {
                            policy_template_id: template_id.to_string(),
                            principal: Some(bound.clone()),
                            resource: None,
                        }),
                        effect: Some("Permit".to_string()),
                        principal: Some(bound.clone()),
                        resource: None,
                        actions: Vec::new(),
                        created_date: created,
                        last_updated_date: created,
                    },
                );
            }
        }
        Resource::Template => {
            while store.policy_templates.len() < MAX_POLICY_TEMPLATES_PER_STORE {
                let id = store.mint_policy_template_id();
                let created = service::state::now();
                store.policy_templates.insert(
                    id.clone(),
                    service::state::PolicyTemplate {
                        policy_template_id: id,
                        policy_store_id: store_id.to_string(),
                        name: None,
                        description: None,
                        statement: SHORT_TEMPLATE.to_string(),
                        created_date: created,
                        last_updated_date: created,
                    },
                );
            }
        }
    }
}

/// A create whose `name` another resource in the store already holds answers the
/// documented 409 `ConflictException`, naming that resource, even when the store
/// has no room for the new record — while the same request with a name nothing
/// holds still earns the 402 capacity refusal, and neither refusal stores
/// anything.
///
/// The rows cross the two writers that mint a named resource with the two
/// capacity dimensions a store has.
#[tokio::test]
async fn a_create_reports_the_taken_name_before_the_store_runs_out_of_room() {
    let (router, engine) = router_with_engine();
    let taken = "taken";
    let free = "free-name";

    let cases: &[(&str, Resource, Capacity)] = &[
        ("CreatePolicy / byte budget", Resource::Policy, Capacity::Bytes),
        ("CreatePolicy / policy count", Resource::Policy, Capacity::Count),
        (
            "CreatePolicyTemplate / byte budget",
            Resource::Template,
            Capacity::Bytes,
        ),
        (
            "CreatePolicyTemplate / template count",
            Resource::Template,
            Capacity::Count,
        ),
    ];

    for (case, resource, capacity) in cases {
        let store_id = store_with_validation_off(&router).await;
        // The resource that holds the name, created through the API.
        let holder = create_one(
            &router,
            *resource,
            &store_id,
            &resource.smallest(),
            Some(taken),
        )
        .await;

        match capacity {
            Capacity::Bytes => fill_to_byte_bound(&router, &engine, &store_id).await,
            Capacity::Count => {
                // The template the seeded template-linked policies follow.
                let template_id =
                    create_one(&router, Resource::Template, &store_id, SHORT_TEMPLATE, None).await;
                seed_to_count_bound(&engine, &store_id, *resource, &template_id);
                let used = cedar_bytes(&engine, &store_id);
                assert!(
                    used + MAX_STATEMENT_BYTES <= MAX_POLICY_BYTES_PER_STORE,
                    "case {case:?}: this row must exercise the COUNT bound alone, but the store \
                     already holds {used} of its {MAX_POLICY_BYTES_PER_STORE} Cedar bytes"
                );
            }
        }

        let (status, body) = attempt_create(&router, *resource, &store_id, taken).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the duplicate-name conflict, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        assert_eq!(
            body["resources"],
            json!([{ "resourceId": holder, "resourceType": resource.modelled_type() }]),
            "case {case:?}: the conflict must name the resource that holds the name, got {body}"
        );

        let (status, body) = attempt_create(&router, *resource, &store_id, free).await;
        assert_eq!(
            status, 402,
            "case {case:?}: a name nothing holds must still earn the capacity refusal, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected the modelled quota error, got {body}"
        );

        assert_eq!(
            id_named(&router, *resource, &store_id, taken).await.as_deref(),
            Some(holder.as_str()),
            "case {case:?}: the name must still resolve to {holder}"
        );
        assert_eq!(
            id_named(&router, *resource, &store_id, free).await,
            None,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// The same ordering on the two updates that take a name: a rename to a name
/// another resource holds answers the 409 `ConflictException` naming that
/// resource, even when the replacement statement has no room in the store, while
/// the same oversized replacement under a free name is still refused for
/// capacity — `UpdatePolicy` with the modelled 402, `UpdatePolicyTemplate`, which
/// models no quota error, with the 400 that names the member — and neither
/// refusal changes the record.
#[tokio::test]
async fn an_update_reports_the_taken_name_before_the_store_runs_out_of_room() {
    let (router, engine) = router_with_engine();
    let taken = "taken";
    let free = "free-name";

    // (case, resource, the status and code the capacity refusal carries)
    let cases: &[(&str, Resource, u16, &str)] = &[
        (
            "UpdatePolicy",
            Resource::Policy,
            402,
            "ServiceQuotaExceededException",
        ),
        (
            "UpdatePolicyTemplate",
            Resource::Template,
            400,
            "ValidationException",
        ),
    ];

    for (case, resource, capacity_status, capacity_code) in cases {
        let store_id = store_with_validation_off(&router).await;
        let original = resource.smallest();
        let holder = create_one(
            &router,
            *resource,
            &store_id,
            &original,
            Some(taken),
        )
        .await;
        // The record the rows update: unnamed, so the rename below is what makes
        // it collide.
        let subject = create_one(&router, *resource, &store_id, &original, None).await;
        fill_to_byte_bound(&router, &engine, &store_id).await;

        let (status, body) = attempt_update(&router, *resource, &store_id, &subject, taken).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the duplicate-name conflict, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        assert_eq!(
            body["resources"],
            json!([{ "resourceId": holder, "resourceType": resource.modelled_type() }]),
            "case {case:?}: the conflict must name the resource that holds the name, got {body}"
        );

        let (status, body) = attempt_update(&router, *resource, &store_id, &subject, free).await;
        assert_eq!(
            status, *capacity_status,
            "case {case:?}: a name nothing holds must still earn the capacity refusal, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *capacity_code,
            "case {case:?}: expected {capacity_code}, got {body}"
        );

        let stored = stored_statement(&router, *resource, &store_id, &subject).await;
        assert_eq!(
            stored, original,
            "case {case:?}: a refused update must leave the record's statement alone"
        );
        assert_eq!(
            id_named(&router, *resource, &store_id, taken).await.as_deref(),
            Some(holder.as_str()),
            "case {case:?}: the name must still resolve to {holder}"
        );
        assert_eq!(
            id_named(&router, *resource, &store_id, free).await,
            None,
            "case {case:?}: a refused update must assign no name"
        );
    }
}
