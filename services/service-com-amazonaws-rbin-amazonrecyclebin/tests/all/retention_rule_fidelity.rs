// SPDX-License-Identifier: CC0-1.0

//! Four fidelity rules of the Recycle Bin retention-rule surface, each asserted over a
//! SET of inputs rather than one recorded request:
//!
//! 1. a rule is minted `pending` and the CLOCK, not a caller, moves it to `available`;
//! 2. a `ResourceNotFoundException` for a rule identifier states
//!    `Recycle Bin Rule '<the requested identifier>' does not exist.`, whether the rule
//!    never existed or was deleted;
//! 3. a refused input constraint names the member path and the requirement it failed,
//!    withholding the rejected value and quoting a `@pattern` as the service publishes
//!    that member's requirement;
//! 4. `CreateRule` refuses more tags than the tags-per-resource quota allows, and
//!    persists nothing when it does.
//!
//! The harness (`router`, `call`, the per-operation helpers) is shared with
//! `retention_rule_lifecycle`, which owns it; every case drives the service through the
//! real router, so the boundary validator, this service's refusal hook and the handlers
//! are exercised exactly as a signed SDK request would exercise them.

use std::time::Duration;

use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    MAX_TAGS_PER_RULE, RULE_STATUS_AVAILABLE, RULE_STATUS_PENDING, STATUS_TRANSITION,
};

use super::retention_rule_lifecycle::{
    RESOURCE_TYPES, RETENTION_VALUE_MAX, RETENTION_VALUE_MIN, UNLOCK_DELAY_MIN, call, create_rule,
    delete_rule, get_rule, list_of, list_rules, lock_configuration, retention, router,
    supported_retention, supported_retention_max, update_rule,
};

/// The envelope every constraint refusal opens with, for a request that violates one
/// constraint.
const ONE_VIOLATION: &str = "1 validation error detected: ";

/// The message of an error body, whichever casing the envelope used: a modelled error
/// renders `Message`, and the framework's pre-handler refusal renders `message`.
fn message_of(body: &serde_json::Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Tags `CreateRule` accepts, as `count` distinct key/value pairs.
///
/// Both members satisfy `Tag`'s `@pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` and its
/// length bounds, so a refusal can only come from the quota under test.
fn tags(count: usize) -> serde_json::Value {
    serde_json::Value::Array(
        (0..count)
            .map(|nth| serde_json::json!({ "Key": format!("k{nth}"), "Value": format!("v{nth}") }))
            .collect(),
    )
}

/// Every rule identifier `ListRules` reports for `resource_type`, in one page.
async fn stored_identifiers(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_type: &str,
) -> Vec<String> {
    let (status, listed) = list_rules(
        router,
        serde_json::json!({ "ResourceType": resource_type, "MaxResults": 1000 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListRules({resource_type}) must answer HTTP 200, got {status} / {listed:?}"
    );
    list_of(&listed, "Rules")
        .iter()
        .map(|summary| summary["Identifier"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE 1: a freshly created rule is `pending` on the create response and on a read
/// taken straight afterwards, and the clock — nothing the caller does — moves it to
/// `available`.
///
/// `rule_status_transitions` owns the span itself (and the update path); this table spans
/// what the rule generalises over in the SHAPE of the created rule: the resource type,
/// the retention period at both published bounds, a tag-level rule, a Region-level rule
/// with exclusion tags, and a locked rule. The status is a fact of the rule's age, so
/// none of them changes it.
#[tokio::test]
async fn a_new_rule_is_pending_and_the_clock_makes_it_available() {
    let router = router();
    let rows = vec![
        (
            "region-level rule at the minimum retention period",
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(RETENTION_VALUE_MIN),
            }),
        ),
        (
            "region-level rule at the maximum retention period its type supports",
            serde_json::json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(supported_retention_max("EBS_VOLUME")),
            }),
        ),
        (
            "tag-level rule with resource tags and rule tags",
            serde_json::json!({
                "ResourceType": "EC2_IMAGE",
                "RetentionPeriod": retention(45),
                "Tags": tags(3),
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
            }),
        ),
        (
            "region-level rule with exclusion tags",
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(7),
                "ExcludeResourceTags": [{ "ResourceTagKey": "scratch" }],
            }),
        ),
        (
            "locked rule",
            serde_json::json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(supported_retention("EBS_VOLUME", 5)),
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
            }),
        ),
    ];

    let mut created_identifiers = Vec::new();
    for (case, body) in &rows {
        let (status, created) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
        );
        assert_eq!(
            created["Status"].as_str(),
            Some(RULE_STATUS_PENDING),
            "case {case:?}: expected a freshly created rule to report \
             Status={RULE_STATUS_PENDING:?}, got {:?}",
            created["Status"]
        );
        let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();

        // The span is never zero: a caller reading straight after the create still sees
        // the transient state.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["Status"].as_str(),
            Some(RULE_STATUS_PENDING),
            "case {case:?}: expected GetRule inside the transition span to report \
             Status={RULE_STATUS_PENDING:?}, got {:?}",
            fetched["Status"]
        );
        created_identifiers.push((case, identifier));
    }

    // Wait out the span the clock owes every one of them, once.
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(200)).await;

    for (case, identifier) in &created_identifiers {
        for read in 1..=2 {
            let (status, fetched) = get_rule(&router, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?} (read {read}): expected HTTP 200 from GetRule, \
                 got {status} / {fetched:?}"
            );
            assert_eq!(
                fetched["Status"].as_str(),
                Some(RULE_STATUS_AVAILABLE),
                "case {case:?} (read {read}): expected Status={RULE_STATUS_AVAILABLE:?} once the \
                 transition is due, got {:?}",
                fetched["Status"]
            );
        }
    }
}

/// RULE 2: every rule-addressed operation answers an identifier no rule has with the
/// not-exist message composed from the identifier the REQUEST named.
///
/// The rows differ in the identifier's form (all-lowercase, all-uppercase, mixed,
/// all-digits) and in why it names no rule (never minted, or deleted), and cover every
/// operation of this service that raises `ResourceNotFoundException` — so the message
/// cannot be keyed to one recorded value, and a deleted rule is indistinguishable from
/// one that never existed.
#[tokio::test]
async fn an_absent_rule_answers_the_not_exist_message_composed_from_the_request() {
    let router = router();

    // A rule that exists, so the store is not simply empty, plus one created and then
    // deleted — the identifier of which must answer exactly like a never-minted one.
    let (status, _) = create_rule(
        &router,
        serde_json::json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(status, 201, "setup: expected HTTP 201 from CreateRule, got {status}");
    let (status, deleted_rule) = create_rule(
        &router,
        serde_json::json!({ "ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(6) }),
    )
    .await;
    assert_eq!(status, 201, "setup: expected HTTP 201 from CreateRule, got {status}");
    let deleted = deleted_rule["Identifier"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, _) = delete_rule(&router, &deleted).await;
    assert_eq!(status, 204, "setup: expected HTTP 204 from DeleteRule, got {status}");

    let rows = [
        ("never minted, all lowercase", "zzzzzzzzzzz".to_string()),
        ("never minted, all uppercase", "ZZZZZZZZZZZ".to_string()),
        ("never minted, mixed case", "0a1B2c3D4e5".to_string()),
        ("never minted, all digits", "99999999999".to_string()),
        ("deleted", deleted.clone()),
    ];

    for (why, identifier) in rows {
        for (operation, (status, body)) in [
            ("GetRule", get_rule(&router, &identifier).await),
            (
                "UpdateRule",
                update_rule(
                    &router,
                    &identifier,
                    serde_json::json!({ "Description": "anything" }),
                )
                .await,
            ),
            ("DeleteRule", delete_rule(&router, &identifier).await),
            (
                "LockRule",
                call(
                    &router,
                    "PATCH",
                    &format!("/rules/{identifier}/lock"),
                    Some(serde_json::json!({
                        "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
                    })),
                )
                .await,
            ),
            (
                "UnlockRule",
                call(
                    &router,
                    "PATCH",
                    &format!("/rules/{identifier}/unlock"),
                    Some(serde_json::json!({})),
                )
                .await,
            ),
        ] {
            let case = format!("{operation} on an identifier {why} ({identifier})");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            // Composed from the request, so the expectation is composed the same way.
            let expected = format!("Recycle Bin Rule '{identifier}' does not exist.");
            assert_eq!(
                message_of(&body),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                message_of(&body)
            );
            // The optional `Reason` of the modelled 404 is not populated on this path,
            // so the body carries the message alone (see `crate::state::rule_not_found`).
            assert_eq!(
                body.get("Reason"),
                None,
                "case {case:?}: expected no Reason member on the not-found body, got {:?}",
                body["Reason"]
            );
        }
    }
}

/// One constraint-refusal row: what was sent, and the clause the service must answer.
struct RefusalCase {
    case: &'static str,
    /// The response the request produced.
    status: u16,
    body: serde_json::Value,
    /// The member path and requirement the message must state, or `None` where the row
    /// only pins the shape of the message (a violation inside a list, whose path
    /// spelling is the framework's).
    clause: Option<String>,
    /// The value the request sent for that member, which the message must NOT echo.
    rejected: &'static str,
}

/// RULE 3: a refused constraint is stated as the member path and the requirement, with
/// the rejected value withheld and a `@pattern` quoted as the service publishes that
/// member's requirement — as the model declares it for a member the request carries in
/// its BODY, and without the whole-value anchors for one bound into the URI PATH, which
/// is matched as one segment of the route.
///
/// The table spans the constraint classes the model uses (enum, `@range` at both ends,
/// `@pattern`), the operations that carry them (`CreateRule`, `UpdateRule`, `GetRule`,
/// `LockRule`), and the depth of the member (top level, nested once, nested twice,
/// inside a list) — including members and operations the issue never cited. An absent
/// required member is a different class and keeps its own `Value null at` wording, which
/// the last row pins.
#[tokio::test]
async fn a_refused_constraint_names_the_member_without_echoing_the_value() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        serde_json::json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(status, 201, "setup: expected HTTP 201 from CreateRule, got {status}");
    let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();

    let enum_days = "Member must satisfy enum value set: [DAYS]";
    let mut rows: Vec<RefusalCase> = Vec::new();

    for (case, unit) in [
        ("CreateRule retention period in weeks", "WEEKS"),
        ("CreateRule retention period in hours", "HOURS"),
    ] {
        let (status, body) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": {
                    "RetentionPeriodValue": 30,
                    "RetentionPeriodUnit": unit,
                },
            }),
        )
        .await;
        rows.push(RefusalCase {
            case,
            status,
            body,
            clause: Some(format!(
                "Value at 'retentionPeriod.retentionPeriodUnit' failed to satisfy constraint: \
                 {enum_days}"
            )),
            rejected: unit,
        });
    }

    for (case, value, requirement, rejected) in [
        (
            "CreateRule retention period one below the minimum",
            RETENTION_VALUE_MIN - 1,
            format!("Member must have value greater than or equal to {RETENTION_VALUE_MIN}"),
            "0",
        ),
        (
            "CreateRule retention period one above the maximum",
            RETENTION_VALUE_MAX + 1,
            format!("Member must have value less than or equal to {RETENTION_VALUE_MAX}"),
            "3651",
        ),
    ] {
        let (status, body) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(value),
            }),
        )
        .await;
        rows.push(RefusalCase {
            case,
            status,
            body,
            clause: Some(format!(
                "Value at 'retentionPeriod.retentionPeriodValue' failed to satisfy constraint: \
                 {requirement}"
            )),
            rejected,
        });
    }

    // `ResourceType` is an enum the issue never cited; its value set comes from the model.
    let (status, body) = create_rule(
        &router,
        serde_json::json!({
            "ResourceType": "EBS_SNAPSHOTS",
            "RetentionPeriod": retention(7),
        }),
    )
    .await;
    // The enum's value set is reported in the model's own member order, which is not a
    // rule of this service, so the clause is checked by its parts rather than verbatim.
    let message = message_of(&body);
    assert!(
        message.contains(
            "Value at 'resourceType' failed to satisfy constraint: \
             Member must satisfy enum value set: ["
        ),
        "case \"CreateRule with a resource type outside the enum\": expected the member path \
         and the enum requirement, got {message:?}"
    );
    for resource_type in RESOURCE_TYPES {
        assert!(
            message.contains(resource_type),
            "case \"CreateRule with a resource type outside the enum\": expected the modelled \
             value {resource_type:?} in the reported value set, got {message:?}"
        );
    }
    rows.push(RefusalCase {
        case: "CreateRule with a resource type outside the enum",
        status,
        body,
        clause: None,
        rejected: "EBS_SNAPSHOTS",
    });

    // `Description` is `@pattern ^[\S ]{0,255}$` and travels in the BODY, so the
    // expression is quoted as the model declares it: a tab is neither `\S` nor a space.
    let description_pattern =
        "Member must satisfy regular expression pattern: ^[\\S ]{0,255}$".to_string();
    let (status, body) = create_rule(
        &router,
        serde_json::json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(7),
            "Description": "tab\tseparated",
        }),
    )
    .await;
    rows.push(RefusalCase {
        case: "CreateRule with a description outside its pattern",
        status,
        body,
        clause: Some(format!(
            "Value at 'description' failed to satisfy constraint: {description_pattern}"
        )),
        rejected: "tab\tseparated",
    });
    let (status, body) = update_rule(
        &router,
        &identifier,
        serde_json::json!({ "Description": "tab\tseparated" }),
    )
    .await;
    rows.push(RefusalCase {
        case: "UpdateRule with a description outside its pattern",
        status,
        body,
        clause: Some(format!(
            "Value at 'description' failed to satisfy constraint: {description_pattern}"
        )),
        rejected: "tab\tseparated",
    });

    // `RuleIdentifier` is `^[0-9a-zA-Z]{11}$` and is bound into the URI PATH, so the
    // expression is quoted spliced into the route's — without the anchor pair.
    for (case, bad_identifier) in [
        ("GetRule with a punctuated identifier", "not-a-valid-id!"),
        ("GetRule with a too-short identifier", "abc"),
    ] {
        let (status, body) = get_rule(&router, bad_identifier).await;
        rows.push(RefusalCase {
            case,
            status,
            body,
            clause: Some(
                "Value at 'identifier' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}"
                    .to_string(),
            ),
            rejected: bad_identifier,
        });
    }

    // `LockRule`'s unlock delay: an enum and a `@range`, both nested twice.
    let (status, body) = call(
        &router,
        "PATCH",
        &format!("/rules/{identifier}/lock"),
        Some(serde_json::json!({
            "LockConfiguration": {
                "UnlockDelay": { "UnlockDelayValue": 10, "UnlockDelayUnit": "WEEKS" },
            },
        })),
    )
    .await;
    rows.push(RefusalCase {
        case: "LockRule with an unlock delay in weeks",
        status,
        body,
        clause: Some(format!(
            "Value at 'lockConfiguration.unlockDelay.unlockDelayUnit' failed to satisfy \
             constraint: {enum_days}"
        )),
        rejected: "WEEKS",
    });
    let (status, body) = call(
        &router,
        "PATCH",
        &format!("/rules/{identifier}/lock"),
        Some(serde_json::json!({
            "LockConfiguration": {
                "UnlockDelay": { "UnlockDelayValue": 31, "UnlockDelayUnit": "DAYS" },
            },
        })),
    )
    .await;
    rows.push(RefusalCase {
        case: "LockRule with an unlock delay past the published maximum",
        status,
        body,
        clause: Some(
            "Value at 'lockConfiguration.unlockDelay.unlockDelayValue' failed to satisfy \
             constraint: Member must have value less than or equal to 30"
                .to_string(),
        ),
        rejected: "31",
    });

    // A violation INSIDE a list: the path is the framework's spelling, so this row pins
    // only that the value is withheld, while the pattern it quotes is asserted here.
    let (status, body) = create_rule(
        &router,
        serde_json::json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(7),
            "Tags": [
                { "Key": "fine", "Value": "fine" },
                { "Key": "bad?key", "Value": "v" },
            ],
        }),
    )
    .await;
    // `TagKey` travels in the BODY, nested inside a list, so its expression is quoted as
    // the model declares it — the member's depth changes nothing.
    // Source: smithy_query(query="shape", shape="TagKey") — @pattern
    // ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$.
    let tag_key_requirement =
        r"Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";
    let stated = message_of(&body);
    assert!(
        stated.ends_with(tag_key_requirement),
        "case \"CreateRule with a tag key outside its pattern\": expected the clause to state \
         {tag_key_requirement:?}, got {stated:?}"
    );
    rows.push(RefusalCase {
        case: "CreateRule with a tag key outside its pattern",
        status,
        body,
        clause: None,
        rejected: "bad?key",
    });

    for RefusalCase {
        case,
        status,
        body,
        clause,
        rejected,
    } in &rows
    {
        assert_eq!(
            *status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {body:?}"
        );
        let code = body["__type"].as_str();
        assert!(
            code.is_none_or(|name| name.ends_with("ValidationException")),
            "case {case:?}: expected the ValidationException code, got {code:?}"
        );
        let message = message_of(body);
        assert!(
            message.starts_with(ONE_VIOLATION),
            "case {case:?}: expected the message to open with {ONE_VIOLATION:?}, got {message:?}"
        );
        if let Some(clause) = clause {
            assert_eq!(
                message,
                format!("{ONE_VIOLATION}{clause}"),
                "case {case:?}: expected {:?}, got {message:?}",
                format!("{ONE_VIOLATION}{clause}")
            );
        }
        assert!(
            !message.contains(rejected),
            "case {case:?}: the rejected value {rejected:?} must not be echoed, got {message:?}"
        );
    }

    // An absent required member is a different refusal class: it echoes no value, so it
    // keeps the framework's own `Value null at '<member>'` wording.
    let (status, body) = create_rule(
        &router,
        serde_json::json!({ "ResourceType": "EBS_SNAPSHOT" }),
    )
    .await;
    let case = "CreateRule without the required retention period";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} / {body:?}"
    );
    let message = message_of(&body);
    assert_eq!(
        message,
        format!("{ONE_VIOLATION}Value null at 'retentionPeriod' failed to satisfy constraint: \
                 Member must not be null"),
        "case {case:?}: expected the absent-member wording, got {message:?}"
    );
}

/// RULE 4: `CreateRule` accepts up to the tags-per-resource quota and refuses anything
/// beyond it with the modelled `ServiceQuotaExceededException` (HTTP 402), persisting
/// nothing when it refuses.
///
/// The rows span the tag count: none, one, the quota itself, one past it, and well past
/// it up to the model's own `TagList` maximum — which is 200, four times the quota, so a
/// request can be inside the model and outside the quota.
#[tokio::test]
async fn create_rule_enforces_the_tags_per_rule_quota() {
    let router = router();
    const RESOURCE_TYPE: &str = "EBS_VOLUME";
    /// The model's `TagList` upper bound, which the quota sits well inside.
    /// Source: smithy_query(query="shape", shape="TagList") — length {min: 0, max: 200}.
    const TAG_LIST_MAX: usize = 200;

    let rows = [
        ("no tags", 0usize, 201u16),
        ("one tag", 1, 201),
        ("one below the quota", MAX_TAGS_PER_RULE - 1, 201),
        ("exactly the quota", MAX_TAGS_PER_RULE, 201),
        ("one past the quota", MAX_TAGS_PER_RULE + 1, 402),
        ("half again the quota", MAX_TAGS_PER_RULE + MAX_TAGS_PER_RULE / 2, 402),
        ("the model's whole TagList bound", TAG_LIST_MAX, 402),
    ];

    for (case, count, expected) in rows {
        let before = stored_identifiers(&router, RESOURCE_TYPE).await;
        let (status, body) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": RESOURCE_TYPE,
                "RetentionPeriod": retention(3),
                "Tags": tags(count),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} ({count} tags): expected HTTP {expected}, got {status} / {body:?}"
        );
        let after = stored_identifiers(&router, RESOURCE_TYPE).await;

        if expected == 201 {
            assert_eq!(
                list_of(&body, "Tags").len(),
                count,
                "case {case:?}: expected the stored rule to carry {count} tags, got {:?}",
                body["Tags"]
            );
            let identifier = body["Identifier"].as_str().unwrap_or_default().to_string();
            // ROUND TRIP: the accepted rule is readable afterwards.
            let (status, fetched) = get_rule(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetRule to find the created rule, \
                 got {status} / {fetched:?}"
            );
            assert_eq!(
                after.len(),
                before.len() + 1,
                "case {case:?}: expected exactly one more stored rule, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            body["Reason"].as_str(),
            Some("SERVICE_QUOTA_EXCEEDED"),
            "case {case:?}: expected reason SERVICE_QUOTA_EXCEEDED, got {:?}",
            body["Reason"]
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused CreateRule must persist nothing, got {after:?}"
        );
        assert!(
            body["Identifier"].is_null(),
            "case {case:?}: a refused CreateRule mints no identifier, got {:?}",
            body["Identifier"]
        );
    }
}
