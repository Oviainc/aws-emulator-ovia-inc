// SPDX-License-Identifier: CC0-1.0

//! The reputation-entity surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: no request creates a reputation entity — one exists for
//! each resource whose reputation the service tracks, referenced by that resource's ARN
//! — and every member of a listed entity is COMPUTED from that resource's record: its
//! customer-managed status record is the status the account last wrote and when it wrote
//! it, its Amazon-SES-managed record is untouched by that write, and its aggregate
//! status is `DISABLED` when either record disables sending, `REINSTATED` when either
//! reinstates it and `ENABLED` otherwise. A listing reports the entities of the account
//! in the Region the call executes in, ordered by reference, narrowed by every key the
//! request's filter names and windowed by its `PageSize`/`NextToken`; input outside a
//! modelled enum, a token this listing never handed out, and a reference naming no
//! resource the call reaches are each refused `BadRequestException` with nothing stored.
//!
//! The tables below span what the rule generalises over: the number of tracked
//! resources (one, several), the form of their names, the Region a resource lives in and
//! the Region a call executes in, each `SendingStatus` a caller may write (and writing
//! one the entity already carries), each `ReputationEntityFilterKey` and the values it
//! filters on, the page size (absent, one, several, zero, larger than the listing) and
//! every refusal leg.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state;

/// The one `ReputationEntityType` the model binds, taken from the crate's own constant
/// so the value is stated once
/// (`smithy_query(query="shape",shape="ReputationEntityType")`: `RESOURCE`).
const RESOURCE: &str = state::REPUTATION_ENTITY_TYPE_RESOURCE;

/// The Region every case signs for unless it is about regional scope.
const DEFAULT_REGION: &str = state::DEFAULT_REGION;

/// The one account this emulator serves, and so the account every ARN it mints and
/// every reference a call may name carries.
const ACCOUNT_ID: &str = "000000000000";

/// Convenience calls for the reputation-entity surface, over the shared harness.
trait ReputationEntities {
    async fn create_tenant(&self, name: &str) -> Reply;
    async fn create_tenant_in(&self, region: &str, name: &str) -> Reply;
    async fn get_tenant(&self, name: &str) -> Reply;
    async fn list_entities(&self, body: Value) -> Reply;
    async fn list_entities_in(&self, region: &str, body: Value) -> Reply;
    async fn set_customer_managed_status(
        &self,
        entity_type: &str,
        reference: &str,
        status: &str,
    ) -> Reply;
    async fn set_customer_managed_status_in(
        &self,
        region: &str,
        entity_type: &str,
        reference: &str,
        status: &str,
    ) -> Reply;
}

impl ReputationEntities for Ses {
    async fn create_tenant(&self, name: &str) -> Reply {
        self.create_tenant_in(DEFAULT_REGION, name).await
    }

    async fn create_tenant_in(&self, region: &str, name: &str) -> Reply {
        self.call_in(
            region,
            "POST",
            "/v2/email/tenants",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn get_tenant(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants/get",
            Some(json!({"TenantName": name})),
        )
        .await
    }

    async fn list_entities(&self, body: Value) -> Reply {
        self.list_entities_in(DEFAULT_REGION, body).await
    }

    async fn list_entities_in(&self, region: &str, body: Value) -> Reply {
        self.call_in(region, "POST", "/v2/email/reputation/entities", Some(body))
            .await
    }

    async fn set_customer_managed_status(
        &self,
        entity_type: &str,
        reference: &str,
        status: &str,
    ) -> Reply {
        self.set_customer_managed_status_in(DEFAULT_REGION, entity_type, reference, status)
            .await
    }

    async fn set_customer_managed_status_in(
        &self,
        region: &str,
        entity_type: &str,
        reference: &str,
        status: &str,
    ) -> Reply {
        // `ReputationEntityReference` is a non-greedy `@httpLabel`, so an ARN reaches
        // the service percent-encoded, the way an SDK escapes a label.
        self.call_in(
            region,
            "PUT",
            &format!(
                "/v2/email/reputation/entities/{entity_type}/{}/customer-managed-status",
                query_escaped(reference)
            ),
            Some(json!({"SendingStatus": status})),
        )
        .await
    }
}

/// One resource a case created, and the reputation entity reference it is tracked
/// under.
struct Tracked {
    /// The name the resource was created with, for reading it back through its own
    /// operation.
    name: String,
    /// `ReputationEntityReference`: the resource's ARN, as the create minted it.
    reference: String,
}

/// Create each of `names` as a tracked resource, and return what it is tracked as.
async fn tracked_resources(ses: &Ses, names: &[&str]) -> Vec<Tracked> {
    let mut tracked = Vec::new();
    for name in names {
        let created = ses.create_tenant(name).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 creating the tracked resource, got {}",
            created.status
        );
        let reference = created
            .member("TenantArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !reference.is_empty(),
            "case {name:?}: the create must mint the resource's ARN, got {:?}",
            created.member("TenantArn")
        );
        tracked.push(Tracked {
            name: (*name).to_string(),
            reference,
        });
    }
    tracked
}

/// The entities a `ListReputationEntities` reply reported, in the order reported.
fn entities(reply: &Reply) -> Vec<Value> {
    reply
        .member("ReputationEntities")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `ReputationEntityReference` of each entity a reply reported, in order.
fn references(reply: &Reply) -> Vec<String> {
    entities(reply)
        .iter()
        .map(|entity| member(entity, "/ReputationEntityReference"))
        .collect()
}

/// The string at `pointer` inside an entity, or `""` when absent.
fn member(entity: &Value, pointer: &str) -> String {
    entity
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The entity a listing reported for `reference`, or `Value::Null` when it reported
/// none.
fn entity_for(reply: &Reply, reference: &str) -> Value {
    entities(reply)
        .into_iter()
        .find(|entity| member(entity, "/ReputationEntityReference") == reference)
        .unwrap_or(Value::Null)
}

/// The references a listing must report for `tracked`: every one of them, ordered by
/// reference.
fn every_reference(tracked: &[Tracked]) -> Vec<String> {
    let mut expected: Vec<String> = tracked
        .iter()
        .map(|resource| resource.reference.clone())
        .collect();
    expected.sort();
    expected
}

#[tokio::test]
async fn every_tracked_resource_has_an_entity_whose_status_records_are_reported() {
    let ses = Ses::new();
    // The names span the documented `TenantName` form: one character, several
    // characters, every documented character class, and the documented 64-character
    // bound — what the resource is called changes nothing about its entity but the
    // reference the entity is reported under.
    let longest = format!(
        "tenant-{}",
        "x".repeat(state::MAX_TENANT_NAME_CHARS - "tenant-".len())
    );
    let tracked =
        tracked_resources(&ses, &["alpha", "b", "Tenant_01-beta", longest.as_str()]).await;

    let listed = ses.list_entities(json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case {:?}: expected HTTP 200 from ListReputationEntities, got {}",
        "a listing of every tracked resource", listed.status
    );
    assert_eq!(
        references(&listed),
        every_reference(&tracked),
        "case {:?}: the listing must report one entity per tracked resource, ordered by \
         reference, got {:?}",
        "a listing of every tracked resource",
        references(&listed)
    );

    for entity in entities(&listed) {
        let case = member(&entity, "/ReputationEntityReference");
        assert_eq!(
            member(&entity, "/ReputationEntityType"),
            RESOURCE,
            "case {case:?}: every entity is of the one modelled type, got {entity:?}"
        );
        assert_eq!(
            member(&entity, "/CustomerManagedStatus/Status"),
            "ENABLED",
            "case {case:?}: a resource nobody has acted on may send, got {entity:?}"
        );
        assert_eq!(
            member(&entity, "/AwsSesManagedStatus/Status"),
            "ENABLED",
            "case {case:?}: no finding was raised, so the Amazon-SES-managed status \
             permits sending, got {entity:?}"
        );
        assert_eq!(
            member(&entity, "/SendingStatusAggregate"),
            "ENABLED",
            "case {case:?}: both records permit sending, so the aggregate does, got \
             {entity:?}"
        );
        for record in ["/CustomerManagedStatus", "/AwsSesManagedStatus"] {
            assert!(
                entity
                    .pointer(&format!("{record}/LastUpdatedTimestamp"))
                    .and_then(Value::as_f64)
                    .is_some(),
                "case {case:?}: {record} must report when it was last written, got \
                 {entity:?}"
            );
        }
        // "The reputation impact reflects the highest impact reputation finding for the
        // entity": with no finding against the entity there is no highest one.
        assert!(
            entity.pointer("/ReputationImpact").is_none(),
            "case {case:?}: an entity with no finding reports no impact, got {entity:?}"
        );
    }
}

#[tokio::test]
async fn a_customer_managed_status_change_is_what_the_listing_reports() {
    let ses = Ses::new();
    let tracked = tracked_resources(&ses, &["alpha", "beta", "gamma"]).await;
    // The resource the table writes to; the other two must come through untouched.
    let target = &tracked[1];

    // One row per `SendingStatus` a caller may write, plus writing the status the entity
    // already carries: the aggregate follows the rule
    // `ReputationEntity$SendingStatusAggregate` states over the two records, and the
    // Amazon-SES-managed record permits sending throughout.
    for (case, status, aggregate) in [
        ("sending prevented for the entity", "DISABLED", "DISABLED"),
        (
            "sending allowed even with active findings",
            "REINSTATED",
            "REINSTATED",
        ),
        ("sending allowed again", "ENABLED", "ENABLED"),
        ("the status the entity already carries", "ENABLED", "ENABLED"),
        ("prevented a second time", "DISABLED", "DISABLED"),
    ] {
        let updated = ses
            .set_customer_managed_status(RESOURCE, &target.reference, status)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 writing the customer-managed status, got \
             {} {:?}",
            updated.status,
            updated.member("message")
        );
        assert!(
            updated.is_empty_body(),
            "case {case:?}: the response shape declares no members, got a body"
        );

        let listed = ses.list_entities(json!({})).await;
        let entity = entity_for(&listed, &target.reference);
        assert_eq!(
            member(&entity, "/CustomerManagedStatus/Status"),
            status,
            "case {case:?}: the listing must report the status that was written, got \
             {entity:?}"
        );
        assert_eq!(
            member(&entity, "/AwsSesManagedStatus/Status"),
            "ENABLED",
            "case {case:?}: writing the customer-managed status leaves the \
             Amazon-SES-managed status unchanged, got {entity:?}"
        );
        assert_eq!(
            member(&entity, "/SendingStatusAggregate"),
            aggregate,
            "case {case:?}: the aggregate must follow both records, got {entity:?}"
        );

        // The write reached one entity only.
        for other in tracked
            .iter()
            .filter(|resource| resource.reference != target.reference)
        {
            let untouched = entity_for(&listed, &other.reference);
            assert_eq!(
                member(&untouched, "/SendingStatusAggregate"),
                "ENABLED",
                "case {case:?}: the entity of {:?} was not written to, got {untouched:?}",
                other.name
            );
        }

        // The round trip through the resource's own read: a tenant's sending status is
        // its entity's aggregate status, so the two views cannot disagree.
        let read = ses.get_tenant(&target.name).await;
        let resource = read.member("Tenant");
        assert_eq!(
            member(&resource, "/SendingStatus"),
            aggregate,
            "case {case:?}: the resource's own read must report the aggregate status, \
             got {resource:?}"
        );

        // Reading changed nothing a later caller sees.
        let again = ses.list_entities(json!({})).await;
        assert_eq!(
            entity_for(&again, &target.reference),
            entity,
            "case {case:?}: two listings must agree, got {:?} then {:?}",
            entity,
            entity_for(&again, &target.reference)
        );
    }
}

/// One row of the filter table: the filter to send, and the references the listing must
/// report for it.
struct FilterCase {
    case: &'static str,
    filter: Value,
    expected: Vec<String>,
}

#[tokio::test]
async fn a_listing_reports_only_the_entities_every_filter_key_keeps() {
    let ses = Ses::new();
    let tracked = tracked_resources(&ses, &["alpha", "beta", "gamma"]).await;
    // One resource per aggregate status, so each `SENDING_STATUS` value keeps exactly
    // one entity.
    for (resource, status) in tracked.iter().zip(["DISABLED", "REINSTATED", "ENABLED"]) {
        let updated = ses
            .set_customer_managed_status(RESOURCE, &resource.reference, status)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 writing the customer-managed status, got {}",
            resource.name, updated.status
        );
    }
    let disabled = tracked[0].reference.clone();
    let reinstated = tracked[1].reference.clone();
    let enabled = tracked[2].reference.clone();
    // The ARN prefix every entity of this account in this Region shares, derived from
    // one of the references rather than written out.
    let shared_prefix = disabled
        .split_once(":tenant/")
        .map(|(prefix, _)| format!("{prefix}:tenant/"))
        .expect("a tenant reference is an ARN naming the tenant resource");

    // One row per `ReputationEntityFilterKey` the model binds and per value it filters
    // on, plus a row with no filter at all and a row naming two keys at once.
    let cases = vec![
        FilterCase {
            case: "no filter at all",
            filter: json!({}),
            expected: every_reference(&tracked),
        },
        FilterCase {
            case: "ENTITY_TYPE, the one type the model binds",
            filter: json!({"ENTITY_TYPE": RESOURCE}),
            expected: every_reference(&tracked),
        },
        FilterCase {
            case: "SENDING_STATUS DISABLED",
            filter: json!({"SENDING_STATUS": "DISABLED"}),
            expected: vec![disabled.clone()],
        },
        FilterCase {
            case: "SENDING_STATUS REINSTATED",
            filter: json!({"SENDING_STATUS": "REINSTATED"}),
            expected: vec![reinstated.clone()],
        },
        FilterCase {
            case: "SENDING_STATUS ENABLED",
            filter: json!({"SENDING_STATUS": "ENABLED"}),
            expected: vec![enabled.clone()],
        },
        FilterCase {
            case: "ENTITY_REFERENCE_PREFIX, a whole reference",
            filter: json!({"ENTITY_REFERENCE_PREFIX": reinstated.clone()}),
            expected: vec![reinstated.clone()],
        },
        FilterCase {
            case: "ENTITY_REFERENCE_PREFIX, the prefix every reference shares",
            filter: json!({"ENTITY_REFERENCE_PREFIX": shared_prefix}),
            expected: every_reference(&tracked),
        },
        FilterCase {
            case: "ENTITY_REFERENCE_PREFIX, a prefix no reference starts with",
            filter: json!({"ENTITY_REFERENCE_PREFIX": "arn:aws:ses:eu-west-1:"}),
            expected: Vec::new(),
        },
        // `ReputationImpact` reflects the highest impact finding for the entity, and
        // nothing raised one, so both impact levels keep nothing.
        FilterCase {
            case: "REPUTATION_IMPACT LOW",
            filter: json!({"REPUTATION_IMPACT": "LOW"}),
            expected: Vec::new(),
        },
        FilterCase {
            case: "REPUTATION_IMPACT HIGH",
            filter: json!({"REPUTATION_IMPACT": "HIGH"}),
            expected: Vec::new(),
        },
        FilterCase {
            case: "two keys at once, which keeps only what satisfies both",
            filter: json!({"ENTITY_TYPE": RESOURCE, "SENDING_STATUS": "DISABLED"}),
            expected: vec![disabled.clone()],
        },
        FilterCase {
            case: "two keys at once that no entity satisfies together",
            filter: json!({
                "SENDING_STATUS": "DISABLED",
                "ENTITY_REFERENCE_PREFIX": enabled.clone(),
            }),
            expected: Vec::new(),
        },
    ];

    for FilterCase {
        case,
        filter,
        expected,
    } in cases
    {
        let listed = ses.list_entities(json!({"Filter": filter})).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListReputationEntities, got {}",
            listed.status
        );
        assert_eq!(
            references(&listed),
            expected,
            "case {case:?}: the listing must report exactly the entities the filter \
             keeps, got {:?}",
            references(&listed)
        );
    }
}

#[tokio::test]
async fn a_filter_outside_the_modelled_enums_is_refused() {
    let ses = Ses::new();
    let _ = tracked_resources(&ses, &["alpha"]).await;

    // One row per way a filter can name something the model does not: a key outside
    // `ReputationEntityFilterKey`, and a value outside the enum each of the other three
    // keys filters on.
    for (case, filter) in [
        ("a filter key the model does not bind", json!({"TENANT_NAME": "alpha"})),
        ("a filter key in the wrong case", json!({"entity_type": RESOURCE})),
        (
            "an ENTITY_TYPE outside ReputationEntityType",
            json!({"ENTITY_TYPE": "TENANT"}),
        ),
        (
            "a SENDING_STATUS outside SendingStatus",
            json!({"SENDING_STATUS": "PAUSED"}),
        ),
        (
            "a SENDING_STATUS in the wrong case",
            json!({"SENDING_STATUS": "disabled"}),
        ),
        (
            "a REPUTATION_IMPACT outside RecommendationImpact",
            json!({"REPUTATION_IMPACT": "MEDIUM"}),
        ),
    ] {
        let refused = ses.list_entities(json!({"Filter": filter})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListReputationEntities, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: the refusal must say what was wrong with the filter, got {:?}",
            refused.message()
        );
    }
}

#[tokio::test]
async fn a_page_walk_reports_every_entity_once_in_reference_order() {
    // The rule spans the number of entities and the page size independently, so the
    // rows vary both: a page size absent, one, several, zero, and larger than the whole
    // listing.
    for (case, stored, page_size) in [
        ("one entity, no page size", 1_usize, None),
        ("one entity, a page size larger than the listing", 1, Some(7)),
        ("several entities, one per page", 5, Some(1)),
        ("several entities, two per page", 5, Some(2)),
        ("several entities, a page size that divides the listing", 4, Some(2)),
        ("several entities, no page size", 5, None),
        ("several entities, a page size of zero", 5, Some(0)),
    ] {
        let ses = Ses::new();
        let names: Vec<String> = (0..stored).map(|index| format!("tenant-{index:02}")).collect();
        let tracked = tracked_resources(
            &ses,
            &names.iter().map(String::as_str).collect::<Vec<&str>>(),
        )
        .await;
        let expected = every_reference(&tracked);

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(resume) = &token {
                body["NextToken"] = json!(resume);
            }
            let listed = ses.list_entities(body).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected HTTP 200 from ListReputationEntities, got {}",
                listed.status
            );
            let page = references(&listed);
            if let Some(size) = page_size.filter(|size| *size > 0) {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page must hold at most PageSize entities, got {page:?}"
                );
            }
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= stored + 1,
                "case {case:?}: the walk must terminate, but it is on page {pages} of a \
                 listing of {stored}"
            );
            let next = listed.member("NextToken");
            token = next.as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the walk must report every entity exactly once, in reference \
             order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_never_handed_out_is_refused() {
    let ses = Ses::new();
    let _ = tracked_resources(&ses, &["alpha", "beta"]).await;

    for (case, token) in [
        ("a token that is not base64 at all", "not a token".to_string()),
        (
            "a base64 token this service never minted",
            ::aws_smithy_types::base64::encode("somewhere else"),
        ),
        (
            "a token that is a position in another listing",
            ::aws_smithy_types::base64::encode(format!(
                "{}alpha",
                state::CONTACT_LISTS_PAGE_TOKEN_TAG
            )),
        ),
    ] {
        let refused = ses.list_entities(json!({"NextToken": token})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListReputationEntities, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
    }
}

#[tokio::test]
async fn an_entity_is_reachable_only_from_the_region_its_resource_lives_in() {
    // One row per Region a resource may be created in: the reference carries that
    // Region, and a call executing in another Region reaches neither the entity's
    // listing nor its status.
    for (case, home, elsewhere) in [
        ("the emulator's default region", "us-east-1", "eu-west-1"),
        ("a region in another continent", "eu-west-1", "us-east-1"),
        ("a region with a numbered suffix", "ap-southeast-2", "us-east-1"),
    ] {
        let ses = Ses::new();
        let created = ses.create_tenant_in(home, "regional").await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating the tracked resource, got {}",
            created.status
        );
        let reference = created
            .member("TenantArn")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The listing in the resource's own Region reports its entity.
        let at_home = ses.list_entities_in(home, json!({})).await;
        assert_eq!(
            references(&at_home),
            vec![reference.clone()],
            "case {case:?}: the entity must be listed in its resource's own Region, got \
             {:?}",
            references(&at_home)
        );

        // A listing executing elsewhere reports no entity at all: the operation reports
        // the account's entities in the Region the call executes in.
        let away = ses.list_entities_in(elsewhere, json!({})).await;
        assert_eq!(
            away.status, 200,
            "case {case:?}: expected HTTP 200 from ListReputationEntities, got {}",
            away.status
        );
        assert_eq!(
            references(&away),
            Vec::<String>::new(),
            "case {case:?}: a listing in {elsewhere:?} must report no entity of \
             {home:?}, got {:?}",
            references(&away)
        );

        // Nor may a write executing elsewhere reach it: the reference is refused the
        // way an unknown one is, and the entity keeps the status it had.
        let refused = ses
            .set_customer_managed_status_in(elsewhere, RESOURCE, &reference, "DISABLED")
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 writing from {elsewhere:?}, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        let unchanged = entity_for(&ses.list_entities_in(home, json!({})).await, &reference);
        assert_eq!(
            member(&unchanged, "/SendingStatusAggregate"),
            "ENABLED",
            "case {case:?}: a refused write must leave the entity as it was, got \
             {unchanged:?}"
        );

        // The write executing in the resource's own Region does reach it.
        let accepted = ses
            .set_customer_managed_status_in(home, RESOURCE, &reference, "DISABLED")
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 writing from {home:?}, got {}",
            accepted.status
        );
        let written = entity_for(&ses.list_entities_in(home, json!({})).await, &reference);
        assert_eq!(
            member(&written, "/SendingStatusAggregate"),
            "DISABLED",
            "case {case:?}: the write in the resource's own Region must land, got \
             {written:?}"
        );
    }
}

#[tokio::test]
async fn a_status_write_outside_the_modelled_input_is_refused_and_stores_nothing() {
    let ses = Ses::new();
    let tracked = tracked_resources(&ses, &["alpha"]).await;
    let reference = tracked[0].reference.clone();

    // One row per way the write can be outside the modelled input: a reference naming
    // no resource, a reference outside the account the call is authenticated for, an
    // entity type outside `ReputationEntityType`, and a status outside `SendingStatus`.
    for (case, entity_type, target, status) in [
        (
            "a reference naming no resource this account holds",
            RESOURCE,
            format!("{reference}-never-created"),
            "DISABLED",
        ),
        (
            "a reference that is not a resource ARN at all",
            RESOURCE,
            "alpha".to_string(),
            "DISABLED",
        ),
        (
            "a reference whose ARN names another account",
            RESOURCE,
            reference.replace(ACCOUNT_ID, "999999999999"),
            "DISABLED",
        ),
        (
            "a reference whose ARN names another service",
            RESOURCE,
            reference.replacen("arn:aws:ses:", "arn:aws:sqs:", 1),
            "DISABLED",
        ),
        (
            "an entity type the model does not bind",
            "TENANT",
            reference.clone(),
            "DISABLED",
        ),
        (
            "a sending status the model does not bind",
            RESOURCE,
            reference.clone(),
            "PAUSED",
        ),
        (
            "a sending status in the wrong case",
            RESOURCE,
            reference.clone(),
            "disabled",
        ),
    ] {
        let refused = ses
            .set_customer_managed_status(entity_type, &target, status)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 writing the customer-managed status, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: the refusal must say what was wrong with the request, got {:?}",
            refused.message()
        );
    }

    // A reference naming no resource is refused by naming what it did not find.
    let unknown = format!("{reference}-never-created");
    let refused = ses
        .set_customer_managed_status(RESOURCE, &unknown, "DISABLED")
        .await;
    assert!(
        refused.message().contains(&unknown),
        "case {:?}: the refusal must name the entity it did not find, got {:?}",
        "a reference naming no resource this account holds",
        refused.message()
    );

    // Not one of the refused writes reached the entity.
    let listed = ses.list_entities(json!({})).await;
    let entity = entity_for(&listed, &reference);
    assert_eq!(
        member(&entity, "/CustomerManagedStatus/Status"),
        "ENABLED",
        "case {:?}: a refused write must leave the entity's status as it was, got {entity:?}",
        "every refused write"
    );
    assert_eq!(
        member(&entity, "/SendingStatusAggregate"),
        "ENABLED",
        "case {:?}: a refused write must leave the aggregate status as it was, got {entity:?}",
        "every refused write"
    );
}
