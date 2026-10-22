// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules this slice of Route 53 Application Recovery
//! Controller readiness follows at its REFUSAL boundaries, and for the two resource
//! families (cells, recovery groups) and the tag surface those refusals guard:
//!
//! 1. A create of a readiness resource is denied ACCOUNT-WIDE, after input validation
//!    and before any write, unless this account is eligible for the feature.
//! 2. A resource-addressed read or delete of a name this account has no record of
//!    answers the modelled `ResourceNotFoundException` (404) with this service's
//!    CONSTANT, resource-agnostic sentence — on every call, and whatever was asked for.
//! 3. An ARN-addressed tagging call for an ARN no stored resource carries answers this
//!    service's `NotFoundException` (404) with its own constant sentence, and writes
//!    nothing.
//! 4. A `ListRules` `ResourceType` filter outside the resource types this API documents
//!    is refused with `ValidationException` (400) rather than ignored.
//! 5. A `NextToken` this service did not mint is refused with its constant
//!    invalid-token sentence, which does not echo the token.
//! 6. A readiness status nothing has audited is `UNKNOWN`, and a cell's readiness
//!    summary names exactly the checks that watch a resource scoped to that cell.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (the harness is shared with
//! `readiness_check_lifecycle.rs`), so only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected: an answer that is not computed from
//! persisted state cannot pass.
//!
//! The tests are tables whose rows differ along what each rule generalises over — which
//! of the four resource kinds is addressed, which operation addresses it, name form and
//! length, which optional members the request carries, tag count and which keys are
//! written or removed, filter value, token spelling, and page size.

use serde_json::{json, Value};
use service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::state::parse_account_eligible;

use crate::readiness_check_lifecycle::{Emulator, Reply};

/// The resource types this API supports, written out from the `ResourceSetType`
/// member's own documentation (model: `smithy_query(query="doc",
/// shape="CreateResourceSetRequest", member="ResourceSetType")`) rather than imported
/// from the implementation, so the test pins the catalog instead of re-asserting it.
/// The issue cited only `AWS::SQS::Queue`; every other entry is one it did not cite.
const DOCUMENTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Stage",
    "AWS::ApiGatewayV2::Stage",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::CloudWatch::Alarm",
    "AWS::EC2::CustomerGateway",
    "AWS::DynamoDB::Table",
    "AWS::EC2::Volume",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Lambda::Function",
    "AWS::MSK::Cluster",
    "AWS::RDS::DBCluster",
    "AWS::Route53::HealthCheck",
    "AWS::SQS::Queue",
    "AWS::SNS::Topic",
    "AWS::SNS::Subscription",
    "AWS::EC2::VPC",
    "AWS::EC2::VPNConnection",
    "AWS::EC2::VPNGateway",
    "AWS::Route53RecoveryReadiness::DNSTargetResource",
];

/// The sentence every not-found answer of a resource-addressed operation carries. A
/// CONSTANT, resource-agnostic text is the rule; this is the recorded example of it,
/// written out here rather than imported so the test pins the string.
const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The same, for the ARN-addressed tagging operations.
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The sentence a list answers for a `NextToken` this service did not mint.
const BAD_TOKEN_MESSAGE: &str = "Invalid pagination token.";

/// The sentence the account-eligibility gate denies a create with.
const ACCOUNT_DENIED_MESSAGE: &str = "This operation is currently unavailable for new accounts.";

/// The readiness of anything nothing has audited, one of the four values the
/// `Readiness` enum declares (model: `smithy_query(query="shape", shape="Readiness")`).
const UNKNOWN: &str = "UNKNOWN";

/// An ARN of an AWS resource a resource set can name, used as a `ResourceIdentifier`.
/// It carries no `/`, so it needs no encoding in a path segment.
const QUEUE_ARN: &str = "arn:aws:sqs:us-west-2:000000000000:q1";

/// The ARN AWS documents for these resources. Route 53 Application Recovery Controller
/// readiness is a GLOBAL service, so the region segment is empty. Written out here
/// rather than taken from the handler's helper, so the test pins the format.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:route53-recovery-readiness::{account}:{resource_type}/{name}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// An ARN as ONE URI path segment: `{ResourceArn}` is a non-greedy `@httpLabel`, so an
/// SDK percent-encodes the ARN's `:` and `/` before sending it.
fn label(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

/// Which of the four readiness resources a request addresses. Every rule below holds
/// for all four, so the tables run over this axis rather than over one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cell,
    RecoveryGroup,
    ReadinessCheck,
    ResourceSet,
}

impl Kind {
    const ALL: [Kind; 4] = [
        Kind::Cell,
        Kind::RecoveryGroup,
        Kind::ReadinessCheck,
        Kind::ResourceSet,
    ];

    /// The collection URI, from the model's `@http` bindings.
    fn collection(self) -> &'static str {
        match self {
            Kind::Cell => "/cells",
            Kind::RecoveryGroup => "/recoverygroups",
            Kind::ReadinessCheck => "/readinesschecks",
            Kind::ResourceSet => "/resourcesets",
        }
    }

    /// The resource-type segment of this kind's ARN.
    fn arn_type(self) -> &'static str {
        match self {
            Kind::Cell => "cell",
            Kind::RecoveryGroup => "recovery-group",
            Kind::ReadinessCheck => "readiness-check",
            Kind::ResourceSet => "resource-set",
        }
    }

    /// The response member carrying this kind's name.
    fn name_field(self) -> &'static str {
        match self {
            Kind::Cell => "cellName",
            Kind::RecoveryGroup => "recoveryGroupName",
            Kind::ReadinessCheck => "readinessCheckName",
            Kind::ResourceSet => "resourceSetName",
        }
    }

    /// The response member carrying this kind's ARN.
    fn arn_field(self) -> &'static str {
        match self {
            Kind::Cell => "cellArn",
            Kind::RecoveryGroup => "recoveryGroupArn",
            Kind::ReadinessCheck => "readinessCheckArn",
            Kind::ResourceSet => "resourceSetArn",
        }
    }

    /// The items member of this kind's list operation, where this emulator implements
    /// one (`ListCells` and `ListRecoveryGroups` are still scaffold stubs with their
    /// own filed issues).
    fn list_items_field(self) -> Option<&'static str> {
        match self {
            Kind::Cell | Kind::RecoveryGroup => None,
            Kind::ReadinessCheck => Some("readinessChecks"),
            Kind::ResourceSet => Some("resourceSets"),
        }
    }

    /// The smallest create request that stores a record under `name`.
    fn create_body(self, name: &str) -> Value {
        match self {
            Kind::Cell => json!({"cellName": name}),
            Kind::RecoveryGroup => json!({"recoveryGroupName": name}),
            Kind::ReadinessCheck => {
                json!({"readinessCheckName": name, "resourceSetName": "watched_set"})
            }
            Kind::ResourceSet => json!({
                "resourceSetName": name,
                "resourceSetType": "AWS::SQS::Queue",
                "resources": [],
            }),
        }
    }
}

/// Assert `reply` is the refusal `status` / `code` / `message` names, and that the
/// message is the service's constant rather than a sentence built from the request.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected error code {code}, got {:?} / {}",
        reply.error_code(),
        reply.body
    );
    let actual = reply.string("message").unwrap_or_default();
    assert_eq!(
        actual, message,
        "case {case:?}: expected the constant message {message:?}, got {actual:?}"
    );
}

/// Create `name` of `kind` and assert the create succeeded.
async fn create(emulator: &Emulator, kind: Kind, name: &str) -> Reply {
    create_with(emulator, kind, kind.create_body(name), name).await
}

async fn create_with(emulator: &Emulator, kind: Kind, body: Value, case: &str) -> Reply {
    let reply = emulator
        .send("POST", kind.collection(), Some(body))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected an eligible account's create to answer HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    reply
}

async fn read(emulator: &Emulator, kind: Kind, name: &str) -> Reply {
    emulator
        .send("GET", &format!("{}/{name}", kind.collection()), None)
        .await
}

/// RULE 1 (the account-eligibility gate): a create of a readiness resource passes input
/// validation and is then DENIED account-wide with HTTP 403 `AccessDeniedException`,
/// writing nothing — so every later read of that name still answers not-found and the
/// lists stay empty. Rows differ along resource kind, name form and length, and which
/// optional members (nested cells, member cells, tags) the request carries.
#[tokio::test]
async fn a_create_is_denied_account_wide_and_stores_nothing() {
    let emulator = Emulator::with_account_eligibility(false);

    for kind in Kind::ALL {
        let mut rows: Vec<(String, String, Value)> = Vec::new();
        for (label, name) in [
            ("the shortest name the pattern admits", "a".to_string()),
            ("a name the issue recorded", "rs_1".to_string()),
            ("a 64-character name, the longest reported", "n".repeat(64)),
        ] {
            rows.push((label.to_string(), name.clone(), kind.create_body(&name)));
        }
        // The same request carrying every optional member it has: the gate does not
        // depend on which members are present.
        let full_name = "with_every_member";
        let mut full = kind.create_body(full_name);
        full["tags"] = json!({"env": "prod"});
        if matches!(kind, Kind::Cell | Kind::RecoveryGroup) {
            full["cells"] = json!([arn("cell", "some_other_cell")]);
        }
        rows.push((
            "every optional member present".to_string(),
            full_name.to_string(),
            full,
        ));

        for (row, name, body) in rows {
            let case = format!("{kind:?} / {row}");
            let denied = emulator.send("POST", kind.collection(), Some(body)).await;
            assert_refused(
                &case,
                &denied,
                403,
                "AccessDeniedException",
                ACCOUNT_DENIED_MESSAGE,
            );

            // Nothing was written: the reader still answers the modelled not-found.
            let read_back = read(&emulator, kind, &name).await;
            assert_refused(
                &format!("{case}: read back after the denial"),
                &read_back,
                404,
                "ResourceNotFoundException",
                NOT_FOUND_MESSAGE,
            );

            if let Some(items_field) = kind.list_items_field() {
                let list = emulator.send("GET", kind.collection(), None).await;
                assert_eq!(
                    list.status, 200,
                    "case {case:?}: expected the list to answer HTTP 200, got {} / {}",
                    list.status, list.body
                );
                let items = list
                    .field(items_field)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    items.is_empty(),
                    "case {case:?}: a denied create must leave the list empty, got {items:?}"
                );
            }
        }
    }
}

/// The same gate the other way round: an account the store says IS eligible creates
/// each of the four resources, and every one of them reads back through its own
/// reader exactly as stored, including the containment a cell derives from the other
/// records, and disappears again on delete. Rows differ along resource kind, the
/// optional members present, and whether the record is contained by another.
#[tokio::test]
async fn an_eligible_account_creates_each_resource_and_reads_it_back() {
    let emulator = Emulator::new();

    // A parent cell naming a cell that does not exist yet, so the child's
    // `ParentReadinessScopes` can only come from the parent's stored record.
    let child_arn = arn("cell", "child_cell");
    create_with(
        &emulator,
        Kind::Cell,
        json!({"cellName": "parent_cell", "cells": [child_arn], "tags": {"tier": "outer"}}),
        "a cell nesting a cell",
    )
    .await;

    for (case, kind, body, name) in [
        (
            "a cell with no nested cells",
            Kind::Cell,
            json!({"cellName": "lonely_cell"}),
            "lonely_cell",
        ),
        (
            "a cell nested inside an existing cell",
            Kind::Cell,
            json!({"cellName": "child_cell"}),
            "child_cell",
        ),
        (
            "a recovery group holding a cell",
            Kind::RecoveryGroup,
            json!({"recoveryGroupName": "group_1", "cells": [arn("cell", "lonely_cell")]}),
            "group_1",
        ),
        (
            "a recovery group holding no cells, with tags",
            Kind::RecoveryGroup,
            json!({"recoveryGroupName": "group_2", "tags": {"env": "test"}}),
            "group_2",
        ),
        (
            "a resource set",
            Kind::ResourceSet,
            Kind::ResourceSet.create_body("set_1"),
            "set_1",
        ),
        (
            "a readiness check",
            Kind::ReadinessCheck,
            Kind::ReadinessCheck.create_body("check_1"),
            "check_1",
        ),
    ] {
        let created = create_with(&emulator, kind, body.clone(), case).await;
        assert_eq!(
            created.string(kind.arn_field()),
            Some(arn(kind.arn_type(), name)),
            "case {case:?}: expected the minted ARN, got {:?}",
            created.string(kind.arn_field())
        );

        let read_back = read(&emulator, kind, name).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected the reader to answer HTTP 200, got {} / {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.string(kind.name_field()).as_deref(),
            Some(name),
            "case {case:?}: expected the reader to report the stored name, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.string(kind.arn_field()),
            created.string(kind.arn_field()),
            "case {case:?}: expected the reader to report the created ARN, got {}",
            read_back.body
        );
        assert_eq!(
            read_back.field("tags"),
            created.field("tags"),
            "case {case:?}: expected the reader to report the created tags, got {}",
            read_back.body
        );
        if let Some(cells) = body.get("cells") {
            assert_eq!(
                read_back.field("cells"),
                Some(cells),
                "case {case:?}: expected the reader to report the submitted cells, got {}",
                read_back.body
            );
        }
    }

    // The containment a cell does NOT submit: it is derived from the records that name
    // it, so the child reports the parent cell and the lonely cell reports the recovery
    // group that holds it.
    for (case, cell, expected) in [
        (
            "a cell nested in a parent cell",
            "child_cell",
            vec![arn("cell", "parent_cell")],
        ),
        (
            "a cell held by a recovery group",
            "lonely_cell",
            vec![arn("recovery-group", "group_1")],
        ),
        ("a cell nobody contains", "parent_cell", vec![]),
    ] {
        let reply = read(&emulator, Kind::Cell, cell).await;
        let scopes: Vec<String> = reply
            .field("parentReadinessScopes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            scopes, expected,
            "case {case:?}: expected parentReadinessScopes {expected:?}, got {scopes:?}"
        );
    }

    // And a delete removes exactly that record, after which the reader answers the
    // modelled not-found again.
    for (kind, name) in [
        (Kind::Cell, "lonely_cell"),
        (Kind::RecoveryGroup, "group_2"),
        (Kind::ReadinessCheck, "check_1"),
        (Kind::ResourceSet, "set_1"),
    ] {
        let case = format!("delete {kind:?} {name}");
        let deleted = emulator
            .send("DELETE", &format!("{}/{name}", kind.collection()), None)
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204, got {} / {}",
            deleted.status, deleted.body
        );
        assert_refused(
            &format!("{case}: read after delete"),
            &read(&emulator, kind, name).await,
            404,
            "ResourceNotFoundException",
            NOT_FOUND_MESSAGE,
        );
    }
}

/// RULE 2: every resource-addressed read or delete of a name this account has no record
/// of answers HTTP 404 `ResourceNotFoundException` with the service's CONSTANT sentence
/// — on every call (the rows are each asked twice), for every operation of the family,
/// and whatever the requested name is, which the message never echoes.
#[tokio::test]
async fn every_resource_addressed_read_or_delete_answers_the_same_constant_not_found() {
    let emulator = Emulator::new();
    // A stored neighbour of each kind, so a 404 is about the named record rather than
    // an empty store.
    for kind in Kind::ALL {
        create(&emulator, kind, "a_neighbour").await;
    }

    // Names differing along form and length — the message is the same for all of them.
    for ghost in ["never_created", "A_NEIGHBOUR", "a", &"g".repeat(64)] {
        let update_body = Some(json!({"resourceSetName": "a_neighbour"}));
        let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("DeleteCell", "DELETE", format!("/cells/{ghost}"), None),
            ("GetCell", "GET", format!("/cells/{ghost}"), None),
            (
                "GetCellReadinessSummary",
                "GET",
                format!("/cellreadiness/{ghost}"),
                None,
            ),
            (
                "GetRecoveryGroup",
                "GET",
                format!("/recoverygroups/{ghost}"),
                None,
            ),
            (
                "DeleteRecoveryGroup",
                "DELETE",
                format!("/recoverygroups/{ghost}"),
                None,
            ),
            (
                "GetReadinessCheck",
                "GET",
                format!("/readinesschecks/{ghost}"),
                None,
            ),
            (
                "DeleteReadinessCheck",
                "DELETE",
                format!("/readinesschecks/{ghost}"),
                None,
            ),
            (
                "UpdateReadinessCheck",
                "PUT",
                format!("/readinesschecks/{ghost}"),
                update_body,
            ),
            (
                "GetReadinessCheckStatus",
                "GET",
                format!("/readinesschecks/{ghost}/status"),
                None,
            ),
            (
                "GetReadinessCheckResourceStatus",
                "GET",
                format!("/readinesschecks/{ghost}/resource/{QUEUE_ARN}/status"),
                None,
            ),
            (
                "GetResourceSet",
                "GET",
                format!("/resourcesets/{ghost}"),
                None,
            ),
            (
                "DeleteResourceSet",
                "DELETE",
                format!("/resourcesets/{ghost}"),
                None,
            ),
        ];

        for (operation, method, uri, body) in cases {
            // Asked TWICE: the answer is a report, so it is the same every time.
            for attempt in 1..=2 {
                let case = format!("{operation} {ghost:?} attempt {attempt}");
                let reply = emulator.send(method, &uri, body.clone()).await;
                assert_refused(
                    &case,
                    &reply,
                    404,
                    "ResourceNotFoundException",
                    NOT_FOUND_MESSAGE,
                );
                let message = reply.string("message").unwrap_or_default();
                assert!(
                    !message.contains(ghost),
                    "case {case:?}: the constant message must not echo the requested name, got {message:?}"
                );
            }
        }

        // The neighbours are untouched by all of that: a refusal writes nothing.
        for kind in Kind::ALL {
            let neighbour = read(&emulator, kind, "a_neighbour").await;
            assert_eq!(
                neighbour.status, 200,
                "case {kind:?} {ghost:?}: the stored neighbour must survive, got {} / {}",
                neighbour.status, neighbour.body
            );
        }
    }
}

/// RULE 3: a tagging call for an ARN no stored resource carries is refused with this
/// service's `NotFoundException` (404) and its constant sentence, and writes nothing.
/// Rows differ along which kind the ghost ARN names, whether the name exists under a
/// DIFFERENT kind, and how far from an ARN of this service the value is.
#[tokio::test]
async fn the_tagging_operations_refuse_an_arn_no_resource_carries() {
    let emulator = Emulator::new();
    // A cell named `twin` exists; the resource-set ARN for `twin` does not.
    create(&emulator, Kind::Cell, "twin").await;
    create_with(
        &emulator,
        Kind::ResourceSet,
        json!({
            "resourceSetName": "tagged_set",
            "resourceSetType": "AWS::SQS::Queue",
            "resources": [],
            "tags": {"keep": "me"},
        }),
        "a tagged resource set",
    )
    .await;

    let ghosts = [
        ("a resource-set ARN nobody created", arn("resource-set", "ghost_rs_1")),
        ("a readiness-check ARN nobody created", arn("readiness-check", "ghost_check_1")),
        ("a cell ARN nobody created", arn("cell", "ghost_cell_1")),
        ("a recovery-group ARN nobody created", arn("recovery-group", "ghost_rg_1")),
        ("a name that exists under another kind", arn("resource-set", "twin")),
        (
            "an ARN of another service",
            QUEUE_ARN.to_string(),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (row, ghost) in ghosts {
        let encoded = label(&ghost);
        let calls: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "ListTagsForResources",
                "GET",
                format!("/tags/{encoded}"),
                None,
            ),
            (
                "TagResource",
                "POST",
                format!("/tags/{encoded}"),
                Some(json!({"tags": {"k": "v"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("/tags/{encoded}?tagKeys=k"),
                None,
            ),
        ];
        for (operation, method, uri, body) in calls {
            let case = format!("{operation} / {row}");
            let reply = emulator.send(method, &uri, body).await;
            assert_refused(
                &case,
                &reply,
                404,
                "NotFoundException",
                TAGGING_NOT_FOUND_MESSAGE,
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let tags = emulator
        .send("GET", &format!("/tags/{}", label(&arn("resource-set", "tagged_set"))), None)
        .await;
    assert_eq!(
        tags.field("tags"),
        Some(&json!({"keep": "me"})),
        "a refused tagging call must not change a stored resource's tags, got {}",
        tags.body
    );
}

/// The tag surface the rule above guards: a tag written through `TagResource` is
/// reported by `ListTagsForResources` AND by the resource's own reader (one store, not
/// two), a key written again is replaced, and `UntagResource` removes exactly the keys
/// it names. Rows run over all four resource kinds and over tag counts, replacement,
/// and removal of a key that is not there.
#[tokio::test]
async fn tags_written_through_tag_resource_are_reported_and_removed() {
    let emulator = Emulator::new();

    for kind in Kind::ALL {
        let name = "tag_subject";
        create(&emulator, kind, name).await;
        let resource_arn = arn(kind.arn_type(), name);
        let tags_uri = format!("/tags/{}", label(&resource_arn));

        // Rows: what is written, and what every reader must report afterwards.
        let steps: Vec<(&str, Value, Value)> = vec![
            (
                "one tag on an untagged resource",
                json!({"env": "prod"}),
                json!({"env": "prod"}),
            ),
            (
                "two more tags, one of them replacing a key",
                json!({"env": "dev", "team": "arc"}),
                json!({"env": "dev", "team": "arc"}),
            ),
        ];
        for (row, written, expected) in steps {
            let case = format!("{kind:?} / {row}");
            let tagged = emulator
                .send("POST", &tags_uri, Some(json!({"tags": written})))
                .await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: expected TagResource to answer HTTP 200, got {} / {}",
                tagged.status, tagged.body
            );
            let listed = emulator.send("GET", &tags_uri, None).await;
            assert_eq!(
                listed.field("tags"),
                Some(&expected),
                "case {case:?}: expected ListTagsForResources to report {expected}, got {}",
                listed.body
            );
            let read_back = read(&emulator, kind, name).await;
            assert_eq!(
                read_back.field("tags"),
                Some(&expected),
                "case {case:?}: expected the resource's own reader to report {expected}, got {}",
                read_back.body
            );
        }

        // Removals: a key that is there, a key that is not, then the rest.
        for (row, keys, expected) in [
            ("a key the resource carries", vec!["env"], json!({"team": "arc"})),
            ("a key it does not carry", vec!["absent"], json!({"team": "arc"})),
            ("every remaining key", vec!["team", "absent"], json!({})),
        ] {
            let case = format!("{kind:?} / untag {row}");
            let query = keys
                .iter()
                .map(|key| format!("tagKeys={key}"))
                .collect::<Vec<_>>()
                .join("&");
            let untagged = emulator
                .send("DELETE", &format!("{tags_uri}?{query}"), None)
                .await;
            assert_eq!(
                untagged.status, 204,
                "case {case:?}: expected UntagResource to answer HTTP 204, got {} / {}",
                untagged.status, untagged.body
            );
            let listed = emulator.send("GET", &tags_uri, None).await;
            assert_eq!(
                listed.field("tags"),
                Some(&expected),
                "case {case:?}: expected the remaining tags {expected}, got {}",
                listed.body
            );
        }
    }
}

/// RULE 4: a `ListRules` `ResourceType` filter outside the resource types this API
/// documents is refused with HTTP 400 `ValidationException`; a supported value is NOT
/// refused as invalid. The supported rows cover every entry of the documented catalog,
/// including entries the issue did not cite.
///
/// The LISTING itself is still the scaffold's 501 — the readiness-rule catalog is AWS
/// data this service's model does not carry, and it is what the filed `ListRules`
/// scaffold issue covers — so a supported filter is asserted to reach that 501 rather
/// than a refusal.
#[tokio::test]
async fn list_rules_refuses_a_resource_type_filter_the_api_does_not_support() {
    let emulator = Emulator::new();

    for (row, value) in [
        ("the value the issue recorded", "not_a_valid_type"),
        ("a supported type with a trailing s", "AWS::SQS::Queues"),
        ("a supported type in the wrong case", "aws::sqs::queue"),
        ("a well-formed type of no AWS service", "AWS::Fake::Thing"),
        ("an empty filter value", ""),
        ("a type of another service's shape", "arn:aws:sqs:::queue"),
    ] {
        let case = format!("invalid filter / {row}");
        let reply = emulator
            .send("GET", &format!("/rules?resourceType={value}"), None)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for an unsupported resource type, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} / {}",
            reply.error_code(),
            reply.body
        );
    }

    for value in DOCUMENTED_RESOURCE_TYPES {
        let case = format!("supported filter / {value}");
        let reply = emulator
            .send("GET", &format!("/rules?resourceType={value}"), None)
            .await;
        assert_ne!(
            reply.status, 400,
            "case {case:?}: a documented resource type must not be refused, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.status, 501,
            "case {case:?}: the rule catalog is not implemented, so a supported filter \
             must reach the scaffold's 501, got {} / {}",
            reply.status, reply.body
        );
    }
}

/// RULE 5: a `NextToken` this service did not mint is refused with HTTP 400
/// `ValidationException` and the service's constant invalid-token sentence, which does
/// not echo the token. Rows differ along token spelling and along which paginated
/// operation is asked.
#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused_with_the_constant_message() {
    let emulator = Emulator::new();
    create(&emulator, Kind::Cell, "token_cell").await;
    create(&emulator, Kind::ResourceSet, "token_set").await;
    create(&emulator, Kind::ReadinessCheck, "token_check").await;

    for token in [
        "not-a-valid-token!!",
        "",
        "zzzz",
        // Hex, but not carrying the tag this service's cursors start with.
        "6e6f70652d686578",
    ] {
        for (operation, uri) in [
            ("ListResourceSets", format!("/resourcesets?nextToken={token}")),
            (
                "ListReadinessChecks",
                format!("/readinesschecks?nextToken={token}"),
            ),
            (
                "GetCellReadinessSummary",
                format!("/cellreadiness/token_cell?nextToken={token}"),
            ),
            (
                "GetReadinessCheckStatus",
                format!("/readinesschecks/token_check/status?nextToken={token}"),
            ),
        ] {
            let case = format!("{operation} / token {token:?}");
            let reply = emulator.send("GET", &uri, None).await;
            assert_refused(&case, &reply, 400, "ValidationException", BAD_TOKEN_MESSAGE);
            let message = reply.string("message").unwrap_or_default();
            assert!(
                token.is_empty() || !message.contains(token),
                "case {case:?}: the constant message must not echo the token, got {message:?}"
            );
        }
    }
}

/// RULE 6, first half: the readiness of a stored check nothing has audited is `UNKNOWN`
/// with no per-resource result, on every call; and a per-resource status is reported
/// only for a resource the check actually watches — by ARN or by the component id — and
/// is the modelled not-found for anything else.
#[tokio::test]
async fn a_stored_checks_readiness_is_unknown_until_an_audit_runs() {
    let emulator = Emulator::new();
    create_with(
        &emulator,
        Kind::ResourceSet,
        json!({
            "resourceSetName": "watched_set",
            "resourceSetType": "AWS::SQS::Queue",
            "resources": [
                {"resourceArn": QUEUE_ARN},
                {"componentId": "component_7", "resourceArn": "arn:aws:sqs:us-west-2:000000000000:q2"},
            ],
        }),
        "the watched resource set",
    )
    .await;
    create(&emulator, Kind::ReadinessCheck, "status_check").await;

    for attempt in 1..=2 {
        let case = format!("GetReadinessCheckStatus attempt {attempt}");
        let reply = emulator
            .send("GET", "/readinesschecks/status_check/status", None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("readiness").as_deref(),
            Some(UNKNOWN),
            "case {case:?}: expected readiness {UNKNOWN}, got {}",
            reply.body
        );
        let resources = reply
            .field("resources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            resources.is_empty(),
            "case {case:?}: nothing was audited, so there is no resource result, got {resources:?}"
        );
    }

    for (row, identifier, expected_status) in [
        ("a resource the check watches, by ARN", QUEUE_ARN.to_string(), 200),
        (
            "a resource the check watches, by component id",
            "component_7".to_string(),
            200,
        ),
        (
            "a resource the check does not watch",
            "arn:aws:sqs:us-west-2:000000000000:q9".to_string(),
            404,
        ),
    ] {
        let case = format!("GetReadinessCheckResourceStatus / {row}");
        let reply = emulator
            .send(
                "GET",
                &format!(
                    "/readinesschecks/status_check/resource/{}/status",
                    label(&identifier)
                ),
                None,
            )
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} / {}",
            reply.status, reply.body
        );
        if expected_status == 200 {
            assert_eq!(
                reply.string("readiness").as_deref(),
                Some(UNKNOWN),
                "case {case:?}: expected readiness {UNKNOWN}, got {}",
                reply.body
            );
            let rules = reply
                .field("rules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                rules.is_empty(),
                "case {case:?}: no rule was evaluated, so no rule result is reported, got {rules:?}"
            );
        } else {
            assert_refused(
                &case,
                &reply,
                404,
                "ResourceNotFoundException",
                NOT_FOUND_MESSAGE,
            );
        }
    }
}

/// RULE 6, second half: a cell's readiness summary names exactly the readiness checks
/// that watch a resource scoped to that cell — its own ARN or the ARN of a cell nested
/// inside it — and nothing else, paginated like every other list.
#[tokio::test]
async fn a_cell_readiness_summary_reports_the_checks_scoped_to_that_cell() {
    let emulator = Emulator::new();
    let child_arn = arn("cell", "inner_cell");
    create_with(
        &emulator,
        Kind::Cell,
        json!({"cellName": "outer_cell", "cells": [child_arn.clone()]}),
        "the containing cell",
    )
    .await;
    create(&emulator, Kind::Cell, "inner_cell").await;
    create(&emulator, Kind::Cell, "unrelated_cell").await;

    // One resource set scoped to the nested cell, one scoped to the outer cell itself,
    // one scoped to nothing — and a check watching each.
    for (set, scopes) in [
        ("inner_set", vec![child_arn.clone()]),
        ("outer_set", vec![arn("cell", "outer_cell")]),
        ("unscoped_set", vec![]),
    ] {
        create_with(
            &emulator,
            Kind::ResourceSet,
            json!({
                "resourceSetName": set,
                "resourceSetType": "AWS::SQS::Queue",
                "resources": [{"resourceArn": QUEUE_ARN, "readinessScopes": scopes}],
            }),
            set,
        )
        .await;
    }
    for (check, set) in [
        ("check_inner", "inner_set"),
        ("check_outer", "outer_set"),
        ("check_unscoped", "unscoped_set"),
    ] {
        create_with(
            &emulator,
            Kind::ReadinessCheck,
            json!({"readinessCheckName": check, "resourceSetName": set}),
            check,
        )
        .await;
    }

    for (case, cell, expected) in [
        (
            "a cell containing a nested cell's checks and its own",
            "outer_cell",
            vec!["check_inner", "check_outer"],
        ),
        ("the nested cell itself", "inner_cell", vec!["check_inner"]),
        ("a cell nothing is scoped to", "unrelated_cell", vec![]),
    ] {
        let reply = emulator
            .send("GET", &format!("/cellreadiness/{cell}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("readiness").as_deref(),
            Some(UNKNOWN),
            "case {case:?}: expected readiness {UNKNOWN}, got {}",
            reply.body
        );
        let reported: Vec<String> = reply
            .field("readinessChecks")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("readinessCheckName"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the summary to name {expected:?}, got {reported:?}"
        );
        for item in reply
            .field("readinessChecks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            assert_eq!(
                item.get("readiness").and_then(Value::as_str),
                Some(UNKNOWN),
                "case {case:?}: expected every summarised check to be {UNKNOWN}, got {item}"
            );
        }
    }

    // And the summary pages: a page is bounded by `maxResults` and carries the cursor
    // the next page resumes from.
    let first = emulator
        .send("GET", "/cellreadiness/outer_cell?maxResults=1", None)
        .await;
    let first_names: Vec<String> = first
        .field("readinessChecks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("readinessCheckName"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        first_names,
        vec!["check_inner".to_string()],
        "expected the first page of one, got {first_names:?}"
    );
    let cursor = first
        .string("nextToken")
        .expect("a page that is full must carry a cursor");
    let second = emulator
        .send(
            "GET",
            &format!("/cellreadiness/outer_cell?maxResults=1&nextToken={cursor}"),
            None,
        )
        .await;
    let second_names: Vec<String> = second
        .field("readinessChecks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("readinessCheckName"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        second_names,
        vec!["check_outer".to_string()],
        "expected the cursor to resume after the first page, got {second_names:?}"
    );
}

/// The eligibility an operator grants at start-up through
/// `state::ACCOUNT_ELIGIBLE_ENV` reads as documented: `1`, `true`, `yes` or `on` in any
/// case grants it and anything else grants nothing rather than being guessed at.
#[test]
fn the_account_eligibility_setting_parses_as_documented() {
    for (setting, expected) in [
        ("1", true),
        ("true", true),
        ("TRUE", true),
        ("Yes", true),
        (" on ", true),
        ("", false),
        ("0", false),
        ("false", false),
        ("no", false),
        ("truthy", false),
        ("2", false),
    ] {
        let parsed = parse_account_eligible(setting);
        assert_eq!(
            parsed, expected,
            "case {setting:?}: expected {expected}, got {parsed}"
        );
    }
}
