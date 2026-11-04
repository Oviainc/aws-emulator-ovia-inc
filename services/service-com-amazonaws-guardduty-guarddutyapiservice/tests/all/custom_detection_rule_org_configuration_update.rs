// SPDX-License-Identifier: CC0-1.0

//! `UpdateCustomDetectionRuleOrgConfiguration`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. An update REPLACES the mode and the account scope of the organization
//!    configuration the account holds for a custom detection rule — "updates the
//!    organization-level configuration for a custom detection rule, including the mode
//!    and include/exclude account lists"
//!    (`smithy_query(query="doc",shape="UpdateCustomDetectionRuleOrgConfiguration")`) —
//!    and `GetCustomDetectionRuleOrgConfiguration` reports exactly what it wrote,
//!    keeping the configuration's `CreatedAt`.
//! 2. `IncludeAccountIds` and `ExcludeAccountIds` are ONE scope ("mutually exclusive",
//!    `smithy_query(query="doc",shape="DetectionRuleOrgConfiguration",member="IncludeAccountIds")`):
//!    a request carrying either side replaces the whole scope and clears the other,
//!    and a request carrying neither leaves the stored scope alone.
//! 3. The configuration is identified by `RuleId` alone — the operation binds `RuleId`
//!    as the URI label and `Mode` to the body, where the read binds `Mode` as a query
//!    parameter (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration")`)
//!    — so an update naming the other mode MOVES the configuration to it, and the mode
//!    it left holds nothing.
//! 4. A change restarts the roll-out the clock owes (`PROCESSING` -> `ACTIVE`) and moves
//!    `UpdatedAt`; an update asking for the state the configuration is already in is a
//!    no-op, which is what `@idempotent` on a PUT means observably.
//! 5. A rule the account holds no organization configuration for — including one
//!    configured through another Region's endpoint — is refused with the modelled 404
//!    `ResourceNotFoundException`, never a hollow success and never an upsert.
//! 6. A request naming both account scopes, and one whose account list carries an entry
//!    that is not an account id, are refused with HTTP 400 under the service's
//!    `InvalidInputException` class without echoing the caller's value, and change
//!    nothing.
//!
//! Every org-configuration request below is made by the organization's delegated
//! administrator, which the operation is available only to
//! (`crate::detector_lifecycle::as_delegated_admin`); the refusal of a caller without
//! that standing is the subject of the `org_configuration_authorization` suite.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    as_delegated_admin, call, call_in, error_code, error_shape, new_router, Router,
};

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`
/// (`smithy_query(operation="ListCustomDetectionRules")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// The create is `POST /custom-detection-rule/org-configuration`; the read and the
/// update are `/custom-detection-rule/org-configuration/{RuleId}`
/// (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration,UpdateCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the regional
/// scope of a configuration is asserted rather than assumed.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The modelled 404 shape the update answers a rule with no organization
/// configuration with (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// `DetectionRuleConfigurationStatus` members a configuration can report here:
/// the roll-out is accepted and then settles (`smithy_query(shape="DetectionRuleConfigurationStatus")`).
const PROCESSING: &str = "PROCESSING";

/// Well-formed account ids (12 decimal digits), the only thing an account scope may
/// carry.
const ACCOUNTS: [&str; 5] = [
    "111122223333",
    "222233334444",
    "333344445555",
    "444455556666",
    "555566667777",
];

/// The ids of the rules GuardDuty publishes, read over the wire so the test cannot
/// drift from the catalogue.
async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call(router, "POST", RULES_PATH, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ruleId"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.len() > 2,
        "list-custom-detection-rules: expected a catalogue of more than two rules, got {ids:?}"
    );
    ids
}

pub(crate) async fn create_org_configuration(router: &Router, body: Value) -> (u16, Value) {
    as_delegated_admin(router, HOME_REGION).await;
    call(router, "POST", ORG_CONFIGURATION_PATH, Some(body)).await
}

pub(crate) async fn get_org_configuration(
    router: &Router,
    rule_id: &str,
    mode: &str,
) -> (u16, Value) {
    as_delegated_admin(router, HOME_REGION).await;
    call_in(
        router,
        HOME_REGION,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={mode}"),
        None,
    )
    .await
}

pub(crate) async fn update_org_configuration(
    router: &Router,
    rule_id: &str,
    body: Value,
) -> (u16, Value) {
    update_org_configuration_in(router, HOME_REGION, rule_id, body).await
}

async fn update_org_configuration_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    body: Value,
) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(
        router,
        region,
        "PUT",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}"),
        Some(body),
    )
    .await
}

/// Create one organization configuration, failing the test if the create did not
/// succeed.
async fn configured(router: &Router, rule_id: &str, body: Value) {
    let mut request = json!({"ruleId": rule_id});
    merge(&mut request, body);
    let (status, response) = create_org_configuration(router, request.clone()).await;
    assert_eq!(
        status, 200,
        "create-org-configuration {request:?}: expected 200, got {status} {response:?}"
    );
}

/// Copy `extra`'s members over `target`'s, so a case's body is written as the members
/// it differs in rather than repeated in full.
fn merge(target: &mut Value, extra: Value) {
    let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) else {
        panic!("merge expects two JSON objects, got {target:?} and {extra:?}");
    };
    for (key, value) in extra {
        target.insert(key.clone(), value.clone());
    }
}

/// The configuration `Get` reports for `(rule_id, mode)`, failing the test if the read
/// did not succeed.
async fn read_configuration(router: &Router, rule_id: &str, mode: &str, case: &str) -> Value {
    let (status, body) = get_org_configuration(router, rule_id, mode).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-org-configuration expected 200, got {status} {body:?}"
    );
    body["configuration"].clone()
}

/// A `Timestamp` member as epoch seconds: restJson1's default timestamp format, so a
/// `createdAt`/`updatedAt` is a JSON number here
/// (`smithy_query(shape="DetectionRuleOrgConfiguration")` declares both as
/// `Timestamp` with no format override).
fn timestamp(configuration: &Value, member: &str, case: &str) -> f64 {
    configuration[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a numeric {member}, got {configuration:?}")
    })
}

/// Assert a response is the modelled error SHAPE: the status, the shape in both
/// `__type` and the `x-amzn-errortype` header, and the shape repeated in the
/// exception's own `type` member.
fn assert_modelled_error(case: &str, status: u16, body: &Value, expected: &str, code: u16) {
    assert_eq!(
        status, code,
        "case {case:?}: expected HTTP {code}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        expected,
        "case {case:?}: expected {expected} in __type, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        expected,
        "case {case:?}: expected {expected} in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(expected),
        "case {case:?}: expected {expected} in the type member, got {body:?}"
    );
}

/// Assert a response is this service's 400 rejection, naming `member` and echoing
/// none of `values`.
fn assert_rejected(case: &str, status: u16, body: &Value, member: &str, values: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected {INVALID_INPUT} in __type, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: a rejection must name the member at fault {member:?}, got {message:?}"
    );
    for value in values {
        assert!(
            !message.contains(value),
            "case {case:?}: a rejection must not echo the caller's value {value:?}, got {message:?}"
        );
    }
}

/// Assert the update answered a bare success: the response shape declares no members
/// (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration")`).
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: update-org-configuration expected 200, got {status} {body:?}"
    );
    assert!(
        body.is_null() || body == &json!({}),
        "case {case:?}: the response declares no members, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: the update replaces the mode and the account scope, and the two
// account lists are one scope.
// ---------------------------------------------------------------------------

/// One row of the replacement table: what the create recorded, what the update asks
/// for, and what the read must then report.
struct Replacement {
    case: &'static str,
    /// The mode and scope the configuration is created with.
    created: Value,
    /// The update's body.
    update: Value,
    /// The mode the configuration must be readable in afterwards.
    mode: &'static str,
    /// The account scope the read must report, as `(includeAccountIds, excludeAccountIds)`.
    include: Vec<&'static str>,
    /// See [`Self::include`].
    exclude: Vec<&'static str>,
}

/// The update writes the mode and the account scope the request names, and the read
/// reports exactly those — over rows differing in which side of the scope the request
/// names, how many accounts it names, whether it names any at all, and whether it
/// moves the mode.
#[tokio::test]
async fn an_update_replaces_the_mode_and_the_account_scope() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    let rows = [
        Replacement {
            case: "one included account grows to three",
            created: json!({"mode": "DRY_RUN", "includeAccountIds": [ACCOUNTS[0]]}),
            update: json!({
                "mode": "DRY_RUN",
                "includeAccountIds": [ACCOUNTS[0], ACCOUNTS[1], ACCOUNTS[2]],
            }),
            mode: "DRY_RUN",
            include: vec![ACCOUNTS[0], ACCOUNTS[1], ACCOUNTS[2]],
            exclude: vec![],
        },
        Replacement {
            case: "an included scope becomes an excluded one",
            created: json!({"mode": "DRY_RUN", "includeAccountIds": [ACCOUNTS[0], ACCOUNTS[1]]}),
            update: json!({"mode": "DRY_RUN", "excludeAccountIds": [ACCOUNTS[3]]}),
            mode: "DRY_RUN",
            include: vec![],
            exclude: vec![ACCOUNTS[3]],
        },
        Replacement {
            case: "an excluded scope becomes an included one",
            created: json!({"mode": "LIVE", "excludeAccountIds": [ACCOUNTS[3], ACCOUNTS[4]]}),
            update: json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[1]]}),
            mode: "LIVE",
            include: vec![ACCOUNTS[1]],
            exclude: vec![],
        },
        Replacement {
            case: "an explicit empty list clears the scope",
            created: json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0], ACCOUNTS[2]]}),
            update: json!({"mode": "LIVE", "includeAccountIds": []}),
            mode: "LIVE",
            include: vec![],
            exclude: vec![],
        },
        Replacement {
            case: "a request naming neither list leaves the stored scope",
            created: json!({"mode": "DRY_RUN", "excludeAccountIds": [ACCOUNTS[4]]}),
            update: json!({"mode": "DRY_RUN"}),
            mode: "DRY_RUN",
            include: vec![],
            exclude: vec![ACCOUNTS[4]],
        },
        Replacement {
            case: "the mode moves and the scope is kept",
            created: json!({"mode": "DRY_RUN", "includeAccountIds": [ACCOUNTS[2]]}),
            update: json!({"mode": "LIVE"}),
            mode: "LIVE",
            include: vec![ACCOUNTS[2]],
            exclude: vec![],
        },
        Replacement {
            case: "the mode moves and the scope flips to five excluded accounts",
            created: json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
            update: json!({"mode": "DRY_RUN", "excludeAccountIds": ACCOUNTS}),
            mode: "DRY_RUN",
            include: vec![],
            exclude: ACCOUNTS.to_vec(),
        },
        Replacement {
            case: "a configuration with no scope gains one",
            created: json!({"mode": "LIVE"}),
            update: json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[4]]}),
            mode: "LIVE",
            include: vec![ACCOUNTS[4]],
            exclude: vec![],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One rule per row, so a row cannot see what another one wrote.
        let rule_id = published[index % published.len()].clone();
        let router = new_router();
        configured(&router, &rule_id, row.created.clone()).await;

        let created_mode = row.created["mode"]
            .as_str()
            .expect("every row names the mode it creates with");
        let before = read_configuration(&router, &rule_id, created_mode, row.case).await;
        let created_at = timestamp(&before, "createdAt", row.case);

        let (status, body) = update_org_configuration(&router, &rule_id, row.update.clone()).await;
        assert_empty_success(row.case, status, &body);

        let after = read_configuration(&router, &rule_id, row.mode, row.case).await;
        assert_eq!(
            after["ruleId"],
            json!(rule_id),
            "case {:?}: expected the rule it configures, got {after:?}",
            row.case
        );
        assert_eq!(
            after["mode"],
            json!(row.mode),
            "case {:?}: expected the mode the update named, got {after:?}",
            row.case
        );
        assert_eq!(
            after["includeAccountIds"],
            json!(row.include),
            "case {:?}: expected includeAccountIds {:?}, got {after:?}",
            row.case,
            row.include
        );
        assert_eq!(
            after["excludeAccountIds"],
            json!(row.exclude),
            "case {:?}: expected excludeAccountIds {:?}, got {after:?}",
            row.case,
            row.exclude
        );
        // An update never re-creates the configuration, so its create instant stands
        // and `UpdatedAt` is at or past it.
        assert!(
            (timestamp(&after, "createdAt", row.case) - created_at).abs() < f64::EPSILON,
            "case {:?}: an update must keep the create instant {created_at}, got {after:?}",
            row.case
        );
        assert!(
            timestamp(&after, "updatedAt", row.case) >= timestamp(&after, "createdAt", row.case),
            "case {:?}: updatedAt must not precede createdAt, got {after:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the configuration is identified by RuleId alone, so the mode moves.
// ---------------------------------------------------------------------------

/// An update naming the other mode MOVES the configuration: it is readable in the mode
/// the update named and the mode it left holds nothing. Asserted in both directions,
/// and the move is itself updatable afterwards.
#[tokio::test]
async fn an_update_moves_the_configuration_to_the_mode_it_names() {
    let published = published_rule_ids(&new_router()).await;

    for (from, to) in [(MODES[0], MODES[1]), (MODES[1], MODES[0])] {
        let case = format!("{from} -> {to}");
        let router = new_router();
        let rule_id = published[0].clone();
        configured(
            &router,
            &rule_id,
            json!({"mode": from, "includeAccountIds": [ACCOUNTS[0]]}),
        )
        .await;
        let before = read_configuration(&router, &rule_id, from, &case).await;

        let (status, body) = update_org_configuration(
            &router,
            &rule_id,
            json!({"mode": to, "excludeAccountIds": [ACCOUNTS[1]]}),
        )
        .await;
        assert_empty_success(&case, status, &body);

        let after = read_configuration(&router, &rule_id, to, &case).await;
        assert_eq!(
            after["mode"],
            json!(to),
            "case {case:?}: expected the mode the update named, got {after:?}"
        );
        assert_eq!(
            after["createdAt"], before["createdAt"],
            "case {case:?}: a move is not a create, so createdAt must stand, got {after:?}"
        );
        let (status, body) = get_org_configuration(&router, &rule_id, from).await;
        assert_modelled_error(
            &format!("{case}: the mode the configuration left"),
            status,
            &body,
            NOT_FOUND,
            404,
        );

        // The moved configuration is still the rule's one configuration, so moving it
        // back is an ordinary update rather than a create.
        let (status, body) = update_org_configuration(&router, &rule_id, json!({"mode": from})).await;
        assert_empty_success(&format!("{case}: moved back"), status, &body);
        let back = read_configuration(&router, &rule_id, from, &case).await;
        assert_eq!(
            back["excludeAccountIds"],
            json!([ACCOUNTS[1]]),
            "case {case:?}: moving the mode must carry the scope with it, got {back:?}"
        );
        let (status, body) = get_org_configuration(&router, &rule_id, to).await;
        assert_modelled_error(
            &format!("{case}: the mode the move-back left"),
            status,
            &body,
            NOT_FOUND,
            404,
        );
    }
}

/// With BOTH modes configured for one rule, an update names the configuration in the
/// mode it carries and leaves the other one exactly as it was.
#[tokio::test]
async fn an_update_touches_only_the_configuration_of_the_mode_it_names() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    let rule_id = published[1].clone();

    for mode in MODES {
        configured(
            &router,
            &rule_id,
            json!({"mode": mode, "includeAccountIds": [ACCOUNTS[0]]}),
        )
        .await;
    }

    for target in MODES {
        let case = format!("both modes configured, updating {target}");
        let other = MODES
            .iter()
            .find(|mode| **mode != target)
            .expect("AssociationMode has two members");
        let other_before = read_configuration(&router, &rule_id, other, &case).await;

        let (status, body) = update_org_configuration(
            &router,
            &rule_id,
            json!({"mode": target, "excludeAccountIds": [ACCOUNTS[2]]}),
        )
        .await;
        assert_empty_success(&case, status, &body);

        let updated = read_configuration(&router, &rule_id, target, &case).await;
        assert_eq!(
            updated["excludeAccountIds"],
            json!([ACCOUNTS[2]]),
            "case {case:?}: expected the scope the update named, got {updated:?}"
        );
        let other_after = read_configuration(&router, &rule_id, other, &case).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: the other mode's configuration must be untouched, got {other_after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a change restarts the roll-out and moves UpdatedAt; an unchanged
// update is a no-op.
// ---------------------------------------------------------------------------

/// A change is rolled out across the organization again, so the configuration
/// re-enters the transitional status the clock settles and its `UpdatedAt` moves; an
/// update asking for the state it is already in changes neither.
#[tokio::test]
async fn a_change_restarts_the_rollout_and_an_unchanged_update_is_a_no_op() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    // Each row is one CHANGE: the body that changes something, from the same
    // starting configuration.
    let changes = [
        (
            "a wider account scope",
            json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0], ACCOUNTS[1]]}),
        ),
        (
            "the other side of the scope",
            json!({"mode": "LIVE", "excludeAccountIds": [ACCOUNTS[0]]}),
        ),
        ("the other mode", json!({"mode": "DRY_RUN"})),
    ];

    for (index, (case, change)) in changes.iter().enumerate() {
        let router = new_router();
        let rule_id = published[index % published.len()].clone();
        configured(
            &router,
            &rule_id,
            json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
        )
        .await;
        let before = read_configuration(&router, &rule_id, "LIVE", case).await;
        let created_at = timestamp(&before, "createdAt", case);

        let (status, body) = update_org_configuration(&router, &rule_id, change.clone()).await;
        assert_empty_success(case, status, &body);

        let mode = change["mode"].as_str().expect("every row names a mode");
        let after = read_configuration(&router, &rule_id, mode, case).await;
        assert!(
            timestamp(&after, "updatedAt", case) > created_at,
            "case {case:?}: a change must move updatedAt past the create instant {created_at}, got {after:?}"
        );
        // The roll-out is a transition the CLOCK owes and the span is never zero, so a
        // read straight after the update still observes the transient status.
        assert_eq!(
            after["status"],
            json!(PROCESSING),
            "case {case:?}: a change restarts the roll-out, got {after:?}"
        );

        // Replaying the SAME update changes nothing: `UpdateCustomDetectionRuleOrgConfiguration`
        // is `@idempotent` (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration")`).
        let (status, body) = update_org_configuration(&router, &rule_id, change.clone()).await;
        assert_empty_success(&format!("{case}: replayed"), status, &body);
        let replayed = read_configuration(&router, &rule_id, mode, case).await;
        assert_eq!(
            replayed["updatedAt"], after["updatedAt"],
            "case {case:?}: an update that changes nothing must not move updatedAt, got {replayed:?} after {after:?}"
        );
        assert_eq!(
            replayed["includeAccountIds"], after["includeAccountIds"],
            "case {case:?}: a replay must leave the scope as it was, got {replayed:?}"
        );
        assert_eq!(
            replayed["excludeAccountIds"], after["excludeAccountIds"],
            "case {case:?}: a replay must leave the scope as it was, got {replayed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: a rule with no organization configuration is the modelled 404.
// ---------------------------------------------------------------------------

/// There is nothing to update for a rule the account holds no organization
/// configuration for, and an update is never an upsert: the answer is the modelled 404
/// and the store is left empty, over rows differing in WHY the rule resolves to
/// nothing.
#[tokio::test]
async fn updating_a_rule_with_no_org_configuration_is_the_modelled_404() {
    let router = new_router();
    let published = published_rule_ids(&router).await;

    // Two well-formed `RuleId`s (`^[a-z0-9]+(-[a-z0-9]+)*$`, `len 1..100`) the
    // catalogue does not publish — a long one and a minimal one — and two PUBLISHED
    // rules nothing configured. The rule configured only through another Region's
    // endpoint is the block after this loop.
    for rule_id in [
        "no-such-published-rule",
        "r1",
        published[2].as_str(),
        published[1].as_str(),
    ] {
        for mode in MODES {
            let case = format!("ruleId {rule_id:?} in mode {mode}, nothing configured");
            let (status, body) =
                update_org_configuration(&router, rule_id, json!({"mode": mode})).await;
            assert_modelled_error(&case, status, &body, NOT_FOUND, 404);

            // Not an upsert: the refused update wrote nothing a read can see.
            let (status, body) = get_org_configuration(&router, rule_id, mode).await;
            assert_modelled_error(
                &format!("{case}: after the refused update"),
                status,
                &body,
                NOT_FOUND,
                404,
            );
        }
    }

    // A configuration is REGIONAL: the Region that created it is the only one that
    // holds it, so another Region's endpoint has nothing to update.
    let rule_id = published[0].as_str();
    configured(
        &router,
        rule_id,
        json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
    )
    .await;
    let before = read_configuration(&router, rule_id, "LIVE", "another Region").await;
    let (status, body) = update_org_configuration_in(
        &router,
        OTHER_REGION,
        rule_id,
        json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[3]]}),
    )
    .await;
    assert_modelled_error("another Region's endpoint", status, &body, NOT_FOUND, 404);
    let after = read_configuration(&router, rule_id, "LIVE", "another Region").await;
    assert_eq!(
        after, before,
        "an update through another Region must leave this Region's configuration alone, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the account scope is refused before anything is written.
// ---------------------------------------------------------------------------

/// Both account scopes at once, and an entry that is not an account id, are refused
/// with this service's 400 class without echoing the caller's value — and the
/// configuration is left exactly as it was.
#[tokio::test]
async fn an_unusable_account_scope_is_refused_and_changes_nothing() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    let rule_id = published[0].clone();
    configured(
        &router,
        &rule_id,
        json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
    )
    .await;
    let before = read_configuration(&router, &rule_id, "LIVE", "baseline").await;

    // Both sides named: the two are one scope, so the request describes two at once.
    let (status, body) = update_org_configuration(
        &router,
        &rule_id,
        json!({
            "mode": "LIVE",
            "includeAccountIds": [ACCOUNTS[1]],
            "excludeAccountIds": [ACCOUNTS[2]],
        }),
    )
    .await;
    assert_rejected(
        "both account scopes",
        status,
        &body,
        "excludeAccountIds",
        &[ACCOUNTS[1], ACCOUNTS[2]],
    );

    // An entry that is 12 characters but not 12 DIGITS, on either side: the modelled
    // `AccountId` is `len 12..12` (`smithy_query(shape="DetectionRuleAccountIds")`),
    // so only a wrong-CHARACTER value reaches the handler and it is the handler that
    // refuses it.
    for bad in ["abcdefghijkl", "1111-2222-33", "aaaa1111bbbb"] {
        for member in ["includeAccountIds", "excludeAccountIds"] {
            let case = format!("{member} entry {bad:?}");
            let mut request = json!({"mode": "LIVE"});
            merge(&mut request, json!({member: [bad]}));
            let (status, body) = update_org_configuration(&router, &rule_id, request).await;
            assert_rejected(&case, status, &body, "accountId", &[bad]);
        }
    }

    // An entry outside `AccountId`'s length is refused at the request BOUNDARY, under
    // the modelled `BadRequestException` envelope rather than this service's class —
    // and it is still a 400 that writes nothing.
    for bad in ["", "11112222333", "1111222233334"] {
        for member in ["includeAccountIds", "excludeAccountIds"] {
            let case = format!("{member} entry {bad:?} outside the modelled length");
            let mut request = json!({"mode": "LIVE"});
            merge(&mut request, json!({member: [bad]}));
            let (status, body) = update_org_configuration(&router, &rule_id, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
        }
    }

    let after = read_configuration(&router, &rule_id, "LIVE", "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused update must change nothing, got {after:?}"
    );
}

/// A `Mode` outside `AssociationMode` and a `RuleId` outside its pattern are refused at
/// the request boundary with HTTP 400, never a 501 and never a hollow success.
#[tokio::test]
async fn a_modelled_constraint_failure_is_refused_with_400() {
    let router = new_router();
    let published = published_rule_ids(&router).await;
    let rule_id = published[0].clone();
    configured(&router, &rule_id, json!({"mode": "LIVE"})).await;

    for (case, path_rule_id, body) in [
        ("a mode outside the enum", rule_id.as_str(), json!({"mode": "SILENT"})),
        ("a lower-cased mode", rule_id.as_str(), json!({"mode": "live"})),
        ("no mode at all", rule_id.as_str(), json!({})),
        ("a ruleId outside its pattern", "Not_A_Rule_Id", json!({"mode": "LIVE"})),
    ] {
        let (status, response) = update_org_configuration(&router, path_rule_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
    }

    // The valid request against the same configuration still succeeds, so the
    // refusals above are about the request and nothing else.
    let (status, response) = update_org_configuration(&router, &rule_id, json!({"mode": "LIVE"})).await;
    assert_empty_success("the well-formed request", status, &response);
}
