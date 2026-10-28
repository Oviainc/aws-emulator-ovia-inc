// SPDX-License-Identifier: CC0-1.0

//! The reputation-management-policy surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`], so
//! routing, deserialization, the handler and serialization are all exercised.
//!
//! The rules these cases pin:
//!
//! * A reputation entity is always under exactly ONE Amazon-SES-managed reputation
//!   management policy, and reports its ARN: the service's own default policy from the
//!   moment the tracked resource exists, and the selected policy once a caller applies
//!   one. The ARN carries the Region the resource lives in and names the policy the
//!   service owns, so the account field is the partition owner rather than the caller's
//!   account. A read and a listing report the same value, because both are computed from
//!   the one resource record.
//! * A reference that resolves to no resource the call reaches names no reputation
//!   entity at all — nothing creates one — so every operation taking a reference refuses
//!   it as INVALID INPUT (`BadRequestException`, HTTP 400) with one sentence naming the
//!   reference it was given, and stores nothing.
//! * A policy value naming none of the predefined policies is invalid input too, and
//!   leaves the policy in force exactly as it was.
//!
//! The tables below span what the rules generalise over: the Region a resource lives in
//! and the Region a call executes in, the form of the resource's name, each predefined
//! policy (applied in turn, and re-applied), applications racing on one entity, and every
//! refusal leg (a reference naming no resource, a reference that is no ARN, another
//! account's, another service's, another Region's, and a policy outside the predefined
//! set).

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The one `ReputationEntityType` the model binds, from the crate's own constant
/// (`smithy_query(query="shape",shape="ReputationEntityType")`: `RESOURCE`).
const RESOURCE: &str = state::REPUTATION_ENTITY_TYPE_RESOURCE;

/// The Region a case signs for unless it is about regional scope.
const DEFAULT_REGION: &str = state::DEFAULT_REGION;

/// Convenience calls for the reputation-entity policy surface, over the shared harness.
trait ReputationPolicies {
    async fn create_tenant_in(&self, region: &str, name: &str) -> Reply;
    async fn get_entity_in(&self, region: &str, entity_type: &str, reference: &str) -> Reply;
    async fn list_entities_in(&self, region: &str) -> Reply;
    async fn put_policy_in(
        &self,
        region: &str,
        entity_type: &str,
        reference: &str,
        policy: &str,
    ) -> Reply;
}

impl ReputationPolicies for Ses {
    async fn create_tenant_in(&self, region: &str, name: &str) -> Reply {
        self.call_in(
            region,
            "POST",
            "/v2/email/tenants",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn get_entity_in(&self, region: &str, entity_type: &str, reference: &str) -> Reply {
        // `ReputationEntityReference` is a non-greedy `@httpLabel`, so an ARN reaches the
        // service percent-encoded, the way an SDK escapes a label.
        self.call_in(
            region,
            "GET",
            &format!(
                "/v2/email/reputation/entities/{}/{}",
                query_escaped(entity_type),
                query_escaped(reference)
            ),
            None,
        )
        .await
    }

    async fn list_entities_in(&self, region: &str) -> Reply {
        self.call_in(
            region,
            "POST",
            "/v2/email/reputation/entities",
            Some(json!({})),
        )
        .await
    }

    async fn put_policy_in(
        &self,
        region: &str,
        entity_type: &str,
        reference: &str,
        policy: &str,
    ) -> Reply {
        self.call_in(
            region,
            "PUT",
            &format!(
                "/v2/email/reputation/entities/{}/{}/policy",
                query_escaped(entity_type),
                query_escaped(reference)
            ),
            Some(json!({"ReputationEntityPolicy": policy})),
        )
        .await
    }
}

/// The `ReputationEntity` a `GetReputationEntity` reply reported.
fn read_entity(reply: &Reply) -> Value {
    reply.member("ReputationEntity")
}

/// The `ReputationManagementPolicy` an entity object reports, or `""` when absent.
fn policy_of(entity: &Value) -> String {
    entity
        .pointer("/ReputationManagementPolicy")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The policy a listing reports for `reference`, or `""` when it reports no such entity.
fn listed_policy(reply: &Reply, reference: &str) -> String {
    reply
        .member("ReputationEntities")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entity| {
            entity.pointer("/ReputationEntityReference").and_then(Value::as_str) == Some(reference)
        })
        .map(|entity| policy_of(&entity))
        .unwrap_or_default()
}

/// Create a tracked resource in `region` and return the reference its entity is held
/// under (the resource's ARN).
async fn tracked_in(ses: &Ses, region: &str, name: &str) -> String {
    let created = ses.create_tenant_in(region, name).await;
    assert_eq!(
        created.status, 200,
        "case {name:?} in {region:?}: expected HTTP 200 creating the tracked resource, got {} {:?}",
        created.status,
        created.message()
    );
    let reference = created
        .member("TenantArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !reference.is_empty(),
        "case {name:?} in {region:?}: the create must mint the resource's ARN, got {:?}",
        created.member("TenantArn")
    );
    reference
}

#[tokio::test]
async fn every_entity_reports_the_service_default_policy_before_anybody_applies_one() {
    // The rows span the Region a tracked resource lives in and the form of its name: the
    // policy in force is the service's default one, in the resource's own Region, from
    // the moment the resource exists.
    for (case, region, name) in [
        ("a resource in the region the call executes in", DEFAULT_REGION, "alpha"),
        ("a resource in another continent", "eu-west-1", "a"),
        (
            "a resource whose name carries every admitted character class",
            "ap-southeast-2",
            "Tenant_9-beta",
        ),
    ] {
        let ses = Ses::new();
        let reference = tracked_in(&ses, region, name).await;
        // The template the service is recorded reporting for the default policy: the
        // partition owner in the account field because the policy is Amazon SES's own,
        // the resource's Region, and the default policy's name.
        let expected = format!("arn:aws:ses:{region}:aws:reputation-policy/standard");

        let read = ses.get_entity_in(region, RESOURCE, &reference).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetReputationEntity, got {} {:?}",
            read.status,
            read.message()
        );
        let entity = read_entity(&read);
        assert_eq!(
            policy_of(&entity),
            expected,
            "case {case:?}: expected the service's default policy {expected:?}, got {:?}",
            policy_of(&entity)
        );

        // The ARN names a policy the SERVICE owns, so it is read back as one of the
        // predefined policies rather than as a resource of the caller's account.
        assert_eq!(
            state::reputation_management_policy_name(&policy_of(&entity)),
            Some(state::DEFAULT_REPUTATION_MANAGEMENT_POLICY),
            "case {case:?}: expected the default policy to be a predefined one, got {:?}",
            policy_of(&entity)
        );

        // A listing of the same call reports the same value: both read the one record.
        let listed = ses.list_entities_in(region).await;
        assert_eq!(
            listed_policy(&listed, &reference),
            expected,
            "case {case:?}: expected the listing to agree with the read, got {:?}",
            listed_policy(&listed, &reference)
        );

        // Reporting only: two reads agree, so reading changed nothing a later caller
        // sees.
        let again = ses.get_entity_in(region, RESOURCE, &reference).await;
        assert_eq!(
            policy_of(&read_entity(&again)),
            expected,
            "case {case:?}: expected a second read to agree, got {:?}",
            policy_of(&read_entity(&again))
        );
    }
}

#[tokio::test]
async fn an_applied_policy_is_what_every_later_read_reports() {
    let ses = Ses::new();
    let reference = tracked_in(&ses, DEFAULT_REGION, "alpha").await;

    // Every predefined policy, applied in turn, then the default one re-applied and one
    // applied twice: the value in force afterwards is the one the call selected, whatever
    // was in force before it.
    let mut selections: Vec<&'static str> = state::REPUTATION_MANAGEMENT_POLICIES.to_vec();
    selections.push(state::DEFAULT_REPUTATION_MANAGEMENT_POLICY);
    selections.push(state::DEFAULT_REPUTATION_MANAGEMENT_POLICY);

    for policy_name in selections {
        let policy = state::reputation_management_policy_arn(DEFAULT_REGION, policy_name);
        let applied = ses
            .put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &policy)
            .await;
        assert_eq!(
            applied.status, 200,
            "case {policy_name:?}: expected HTTP 200 from UpdateReputationEntityPolicy, got {} {:?}",
            applied.status,
            applied.message()
        );
        assert!(
            applied.is_empty_body(),
            "case {policy_name:?}: the write reports no members, got {:?}",
            applied.member("ReputationEntity")
        );

        let entity = read_entity(&ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await);
        assert_eq!(
            policy_of(&entity),
            policy,
            "case {policy_name:?}: expected the applied policy to read back, got {:?}",
            policy_of(&entity)
        );
        assert_eq!(
            listed_policy(&ses.list_entities_in(DEFAULT_REGION).await, &reference),
            policy,
            "case {policy_name:?}: expected the listing to report the applied policy, got {:?}",
            listed_policy(&ses.list_entities_in(DEFAULT_REGION).await, &reference)
        );

        // Applying a policy moves no status record, so the aggregate status is untouched.
        assert_eq!(
            entity
                .pointer("/SendingStatusAggregate")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "ENABLED",
            "case {policy_name:?}: a policy write must not move a status, got {entity:?}"
        );
    }
}

#[tokio::test]
async fn policy_writes_racing_on_one_entity_all_land_and_one_of_them_is_in_force() {
    let ses = Ses::new();
    let reference = tracked_in(&ses, DEFAULT_REGION, "alpha").await;
    let raced: Vec<String> = ["strict", "none"]
        .into_iter()
        .map(|name| state::reputation_management_policy_arn(DEFAULT_REGION, name))
        .collect();

    // Two applications released together on the one entity: the store serialises them, so
    // each is answered 200 and the value in force afterwards is one of the two — never a
    // third value, and never the one that was in force before.
    let (first, second) = tokio::join!(
        ses.put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &raced[0]),
        ses.put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &raced[1])
    );
    for (case, reply) in [("the first racer", &first), ("the second racer", &second)] {
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status,
            reply.message()
        );
    }

    let settled = policy_of(&read_entity(
        &ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await,
    ));
    assert!(
        raced.contains(&settled),
        "case {:?}: expected one of the raced policies {raced:?}, got {settled:?}",
        "two racing applications"
    );

    // A solo application after the race is what every later read reports, so the race
    // left the entity writable.
    let last = state::reputation_management_policy_arn(
        DEFAULT_REGION,
        state::DEFAULT_REPUTATION_MANAGEMENT_POLICY,
    );
    ses.put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &last)
        .await;
    assert_eq!(
        policy_of(&read_entity(
            &ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await
        )),
        last,
        "case {:?}: expected the solo application to settle the policy, got {:?}",
        "after the race",
        policy_of(&read_entity(
            &ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await
        ))
    );
}

#[tokio::test]
async fn a_reference_that_reaches_no_resource_is_invalid_input_on_the_read_and_the_write() {
    let ses = Ses::new();
    let reference = tracked_in(&ses, DEFAULT_REGION, "alpha").await;
    let account = emulator_server::emulator_account_id();
    let policy = state::reputation_management_policy_arn(
        DEFAULT_REGION,
        state::DEFAULT_REPUTATION_MANAGEMENT_POLICY,
    );

    // One row per way a reference can reach nothing: a sibling ARN of a resource that was
    // never created, a value that is no ARN at all, an ARN of another account, an ARN of
    // another service, and an ARN of another Region. Each names no entity, so each is
    // invalid input with the one sentence naming the reference that was given.
    for (case, target) in [
        (
            "a reference naming no resource this account holds",
            format!("{reference}-never-created"),
        ),
        ("a reference that is no ARN at all", "alpha".to_string()),
        (
            "a reference whose ARN names another account",
            reference.replace(account, "999999999999"),
        ),
        (
            "a reference whose ARN names another service",
            reference.replacen("arn:aws:ses:", "arn:aws:sqs:", 1),
        ),
        (
            "a reference whose ARN names another Region",
            reference.replace(DEFAULT_REGION, "eu-west-1"),
        ),
    ] {
        for (operation, refused) in [
            ("GetReputationEntity", ses.get_entity_in(DEFAULT_REGION, RESOURCE, &target).await),
            (
                "UpdateReputationEntityPolicy",
                ses.put_policy_in(DEFAULT_REGION, RESOURCE, &target, &policy)
                    .await,
            ),
        ] {
            assert_eq!(
                refused.status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {} {:?}",
                refused.status,
                refused.message()
            );
            assert_eq!(
                refused.error_code(),
                "BadRequestException",
                "case {case:?} on {operation}: expected BadRequestException, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.message(),
                format!("ReputationEntity <{target}> does not exist"),
                "case {case:?} on {operation}: expected the refusal to name the reference it \
                 was given, got {:?}",
                refused.message()
            );
        }
    }

    // Not one refused call touched the entity that does exist, and none minted one.
    let entity = read_entity(&ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await);
    assert_eq!(
        policy_of(&entity),
        policy,
        "case {:?}: a refused call must leave the policy in force as it was, got {:?}",
        "every refused call",
        policy_of(&entity)
    );
    assert_eq!(
        ses.list_entities_in(DEFAULT_REGION)
            .await
            .member("ReputationEntities")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {:?}: a refused call must mint no entity",
        "every refused call"
    );
}

#[tokio::test]
async fn a_policy_outside_the_predefined_set_is_refused_and_changes_nothing() {
    let ses = Ses::new();
    let reference = tracked_in(&ses, DEFAULT_REGION, "alpha").await;
    let in_force = state::reputation_management_policy_arn(DEFAULT_REGION, "strict");
    ses.put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &in_force)
        .await;

    // "Reputation management policies are Amazon Web Services Amazon SES-managed
    // (predefined policies). You can select from none, standard, and strict policies"
    // (`smithy_query(query="doc",shape="UpdateReputationEntityPolicy")`): the rows span
    // the ways a value can name none of them — a policy name nobody predefined, a
    // predefined name in the wrong case, the bare name rather than its ARN, another
    // resource type of this service, another service's ARN, an ARN whose account is the
    // caller's rather than the service's, and text that is no ARN at all.
    for (case, policy) in [
        (
            "a policy name nobody predefined",
            state::reputation_management_policy_arn(DEFAULT_REGION, "lenient"),
        ),
        (
            "a predefined name in the wrong case",
            state::reputation_management_policy_arn(DEFAULT_REGION, "STANDARD"),
        ),
        (
            "the bare policy name rather than its ARN",
            state::DEFAULT_REPUTATION_MANAGEMENT_POLICY.to_string(),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:ses:{DEFAULT_REGION}:aws:configuration-set/standard"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{DEFAULT_REGION}:aws:reputation-policy/standard"),
        ),
        (
            "an ARN whose account is the caller's rather than the service's",
            format!(
                "arn:aws:ses:{DEFAULT_REGION}:{}:reputation-policy/standard",
                emulator_server::emulator_account_id()
            ),
        ),
        ("text that is no ARN at all", "standard-policy".to_string()),
        (
            "an ARN whose Region field is no Region label",
            format!(
                "arn:aws:ses:{}:aws:reputation-policy/standard",
                "r".repeat(64)
            ),
        ),
    ] {
        let refused = ses
            .put_policy_in(DEFAULT_REGION, RESOURCE, &reference, &policy)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            refused.status,
            refused.message()
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.message(),
            state::invalid_reputation_management_policy(&policy),
            "case {case:?}: expected the refusal to name the policy it was given, got {:?}",
            refused.message()
        );
        assert_eq!(
            policy_of(&read_entity(
                &ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await
            )),
            in_force,
            "case {case:?}: a refused write must leave the policy in force as it was, got {:?}",
            policy_of(&read_entity(
                &ses.get_entity_in(DEFAULT_REGION, RESOURCE, &reference).await
            ))
        );
    }

    // An entity type outside `ReputationEntityType` is invalid input on the write too,
    // and stores nothing.
    for (case, entity_type) in [
        ("an entity type the model does not bind", "TENANT"),
        ("the modelled entity type in the wrong case", "resource"),
    ] {
        let refused = ses
            .put_policy_in(DEFAULT_REGION, entity_type, &reference, &in_force)
            .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            refused.status,
            refused.error_code()
        );
    }
}
