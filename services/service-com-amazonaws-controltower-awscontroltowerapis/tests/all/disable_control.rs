// SPDX-License-Identifier: CC0-1.0

//! `DisableControl`, end to end over HTTP, with `GetEnabledControl`,
//! `ListControlOperations` and `ListTagsForResource` reading back what it wrote.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * for ANY enablement the account holds, whatever control, target, parameter list and
//!   tag map it carries, `DisableControl` starts exactly ONE asynchronous
//!   `DISABLE_CONTROL` operation against it and returns that operation's
//!   `operationIdentifier`: the enablement stays reportable, `UNDER_CHANGE`, while the
//!   teardown is in flight, and once the clock has made the operation due the record is
//!   `SUCCEEDED` with a stable `endTime` and the enablement — and its tags — are gone, so
//!   the pair enables again as a NEW resource;
//! * the enablement a request acts on is the one it NAMES, by `enabledControlIdentifier`
//!   (the ARN `EnableControl` minted), by the `controlIdentifier`/`targetIdentifier` pair,
//!   or by all three naming one enablement — in either of the control's two published ARN
//!   forms. A request naming neither form, or whose members name different enablements, is
//!   `ValidationException` (400); one naming an enablement the account does not hold is
//!   `ResourceNotFoundException` (404); one overlapping an operation still changing the
//!   control is `ConflictException` (409), and a repeat while its own teardown is in
//!   flight is refused the same way, because the model gives this operation no
//!   `@idempotent` trait;
//! * every refusal writes nothing, and a teardown removes only its OWN enablement: every
//!   sibling stays enabled, keeps its tags, and every other operation record keeps its own
//!   type and status.
//!
//! The contract comes from `smithy_query(query="operation", operation="DisableControl")` on
//! this crate's `smithy-model/`: all three input members are OPTIONAL
//! (`controlIdentifier`/`targetIdentifier` are `@length` 20..2048 with pattern
//! `^arn:aws[0-9a-zA-Z_\-:\/]+$`, `enabledControlIdentifier` is an `Arn` of the same
//! shape), the output's `operationIdentifier` is 36 characters of the UUID pattern, the
//! operation carries NO `@idempotent` trait, and `ValidationException` (400),
//! `ResourceNotFoundException` (404) and `ConflictException` (409) are among its modelled
//! errors. The enum values written out here come from
//! `smithy_query(query="shape", shape="ControlOperationType,ControlOperationStatus,\
//! EnablementStatus")`: `ENABLE_CONTROL` | `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` |
//! `RESET_ENABLED_CONTROL`, `SUCCEEDED` | `FAILED` | `IN_PROGRESS` and `SUCCEEDED` |
//! `FAILED` | `UNDER_CHANGE`. The controls the rows enable, and the parameter keys they
//! configure them with, are read off `state::CONTROL_CATALOG` — the catalog
//! `EnableControl` resolves a `controlIdentifier` against — rather than written out by
//! hand, so no row can drift from what this emulator publishes.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    catalog_arn, control, control_operation, control_operations, details, enable, last_operation,
    member, ou, status_summary_status as status,
};
use crate::front_door_validation::arn_of_length;
use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, past_the_settle_span, set_up_emulator,
    simple_manifest, Answer,
};
use crate::resource_tags::{read_tags, reported_tags};

/// POST `/disable-control` with exactly the members `body` carries: every member of this
/// input is optional, so a case sends the ones its row names and no others.
async fn disable(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    call(router, "/disable-control", body).await
}

/// POST `/get-enabled-control` without asserting the answer, for the cases whose subject is
/// whether the enablement still exists.
async fn get_enabled_control(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": identifier }),
    )
    .await
}

/// The tag map a row enables its control with, as the key-ordered map
/// [`reported_tags`] hands back, so a case compares the two directly. A row that omits the
/// member carries no tags.
fn enabled_with_tags(row: &Row) -> std::collections::BTreeMap<String, String> {
    row.tags
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|members| {
            members
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `DISABLE_CONTROL` record the log holds, so a case can assert that a refusal
/// appended none.
async fn teardown_records(router: &MultiServiceRouter) -> Vec<serde_json::Value> {
    control_operations(
        router,
        serde_json::json!({ "controlOperationTypes": ["DISABLE_CONTROL"] }),
    )
    .await
}

/// The catalog controls a caller may enable that declare NO enablement parameters, in
/// catalog order.
fn enablable_without_parameters() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && control.parameters.is_empty())
        .collect()
}

/// The `index`th enablable control declaring no parameters, wrapping round the catalog so a
/// row never depends on the catalog's length.
fn plain_control(index: usize) -> &'static CatalogControl {
    let plain = enablable_without_parameters();
    assert!(
        !plain.is_empty(),
        "the catalog must publish a control a caller may enable that declares no parameters"
    );
    plain[index % plain.len()]
}

/// A control the catalog publishes WITH parameters, which is what a configured row enables.
fn parameterized_control() -> &'static CatalogControl {
    let parameterized: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && !control.parameters.is_empty())
        .collect();
    assert!(
        !parameterized.is_empty(),
        "the catalog must publish an enablable control that declares parameters"
    );
    parameterized[0]
}

/// A catalog control a caller may NOT enable, so a case can name a real control that no
/// enablement of this account can exist for.
fn mandatory_control() -> &'static CatalogControl {
    let mandatory: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| !control.enablable)
        .collect();
    assert!(
        !mandatory.is_empty(),
        "the catalog must publish a mandatory control a caller may not enable"
    );
    mandatory[0]
}

/// A `parameters` list over the first `count` keys the control declares, each carrying the
/// `Document` value at the same position of `values` (cycled), so a row configures a control
/// only with keys it declares while still varying the value's document kind.
fn parameters_of(
    control: &CatalogControl,
    count: usize,
    values: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::Value::Array(
        control
            .parameters
            .iter()
            .take(count)
            .enumerate()
            .map(|(index, key)| {
                serde_json::json!({ "key": key, "value": values[index % values.len()] })
            })
            .collect(),
    )
}

/// How a row NAMES the enablement it tears down: the addressing forms the model's three
/// optional input members allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// `enabledControlIdentifier` alone — the ARN `EnableControl` minted.
    Arn,
    /// `controlIdentifier` + `targetIdentifier`, the control named by its control ARN.
    ControlPair,
    /// The same pair, with the control named by its AWS Control Catalog ARN.
    CatalogPair,
    /// All three members, naming one enablement.
    ArnAndPair,
}

/// The `DisableControl` body naming `arn` — the enablement of `entry` on `target` — in
/// this addressing form.
fn naming(
    address: Address,
    arn: &str,
    entry: &CatalogControl,
    target: &str,
) -> serde_json::Value {
    match address {
        Address::Arn => serde_json::json!({ "enabledControlIdentifier": arn }),
        Address::ControlPair => serde_json::json!({
            "controlIdentifier": control(entry.name),
            "targetIdentifier": target,
        }),
        Address::CatalogPair => serde_json::json!({
            "controlIdentifier": catalog_arn(entry.catalog_id),
            "targetIdentifier": target,
        }),
        Address::ArnAndPair => serde_json::json!({
            "enabledControlIdentifier": arn,
            "controlIdentifier": control(entry.name),
            "targetIdentifier": target,
        }),
    }
}

/// One row of the teardown table.
struct Row {
    case: &'static str,
    control: &'static CatalogControl,
    target: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it.
    parameters: Option<serde_json::Value>,
    /// The `tags` member `EnableControl` is given; `None` omits it.
    tags: Option<serde_json::Value>,
    address: Address,
}

/// The teardown table: the rows differ along what the rule generalises over — the control,
/// the target, the parameter list (absent, empty, one key, every key the control declares,
/// over several document kinds), the tag map (absent, empty, one, several) and the form the
/// request NAMES the enablement in. Every row is a different control/target pair, because a
/// control is enabled at most once per target.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no parameters and no tags, named by its own ARN",
            control: plain_control(0),
            target: ou("d001"),
            parameters: None,
            tags: None,
            address: Address::Arn,
        },
        Row {
            case: "an empty parameter list and an empty tag map, named by the control ARN pair",
            control: plain_control(1),
            target: ou("d002"),
            parameters: Some(serde_json::json!([])),
            tags: Some(serde_json::json!({})),
            address: Address::ControlPair,
        },
        Row {
            case: "one declared parameter and one tag, named by the catalog ARN pair",
            control: parameterized_control(),
            target: ou("d003"),
            parameters: Some(parameters_of(
                parameterized_control(),
                1,
                &[serde_json::json!("us-west-2")],
            )),
            tags: Some(serde_json::json!({ "owner": "platform" })),
            address: Address::CatalogPair,
        },
        Row {
            case: "every declared parameter and several tags, named by all three members",
            control: parameterized_control(),
            target: ou("d004"),
            parameters: Some(parameters_of(
                parameterized_control(),
                parameterized_control().parameters.len(),
                &[
                    serde_json::json!(["us-west-2", "eu-west-1"]),
                    serde_json::json!("arn:aws:iam::000000000000:role/r"),
                    serde_json::json!({ "inner": { "deeper": [1, 2] } }),
                ],
            )),
            tags: Some(serde_json::json!({ "env": "test", "cost-centre": "4711" })),
            address: Address::ArnAndPair,
        },
        Row {
            case: "another plain control with one tag, named by its own ARN",
            control: plain_control(2),
            target: ou("d005"),
            parameters: None,
            tags: Some(serde_json::json!({ "team": "governance" })),
            address: Address::Arn,
        },
    ]
}

/// `EnableControl` for one row, naming the control by its control ARN, handing back
/// `(arn, operationIdentifier)`.
async fn enable_row(router: &MultiServiceRouter, row: &Row) -> (String, String) {
    let mut body = serde_json::json!({
        "controlIdentifier": control(row.control.name),
        "targetIdentifier": row.target,
    });
    if let Some(parameters) = &row.parameters {
        body["parameters"] = parameters.clone();
    }
    if let Some(tags) = &row.tags {
        body["tags"] = tags.clone();
    }
    enable(router, body).await
}

/// Assert that `refused` is the status and error code the rule names and that it minted
/// nothing: a refusal starts no operation.
fn assert_refused(case: &str, refused: &Answer, expected_status: u16, expected_code: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (expected_status, Some(expected_code)),
        "case {case:?}: expected {expected_status} {expected_code}, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused teardown must mint no operation, got {:?}",
        refused.body
    );
}

/// RULE: every enablement tears down through its own asynchronous `DISABLE_CONTROL`
/// operation — refused while the enablement is still being applied, refused again while
/// that teardown is in flight, terminal once the clock makes it due, and leaving the pair
/// enablable again as a NEW resource. Asserted over the whole table, so the rule holds
/// whatever control, target, parameter list and tag map the enablement carries and whichever
/// form the request names it in.
#[tokio::test]
async fn every_enablement_tears_down_through_its_own_operation() {
    let router = set_up_emulator().await;
    let rows = rows();

    // case, the minted EnabledControl ARN, the ENABLE_CONTROL operation's identifier.
    let mut enabled: Vec<(&'static str, String, String)> = Vec::new();
    for row in &rows {
        let (arn, enable_operation) = enable_row(&router, row).await;
        let case = row.case;

        // `EnableControl` "starts an asynchronous operation", so the enablement is still
        // changing and a teardown would leave it half applied. Probed IMMEDIATELY after
        // this row's own enable, so the assertion does not depend on how long the rest of
        // the table takes.
        let too_soon = disable(&router, naming(row.address, &arn, row.control, &row.target)).await;
        assert_refused(case, &too_soon, 409, "ConflictException");
        assert!(
            teardown_records(&router).await.is_empty(),
            "case {case:?}: a refused teardown must append no DISABLE_CONTROL record, got {:?}",
            teardown_records(&router).await
        );
        // The refusal left the enable's own record alone.
        let record = control_operation(&router, &enable_operation).await;
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("ENABLE_CONTROL"), Some("IN_PROGRESS")),
            "case {case:?}: a refused teardown must not disturb the enable's record, got \
             {record:?}"
        );

        enabled.push((case, arn, enable_operation));
    }

    // Nothing sleeps server-side: the clock makes the enables due, and the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    // case, ARN, the DISABLE_CONTROL operation's identifier.
    let mut teardowns: Vec<(&'static str, String, String)> = Vec::new();
    for (row, (case, arn, enable_operation)) in rows.iter().zip(&enabled) {
        let body = naming(row.address, arn, row.control, &row.target);
        let disabled = disable(&router, body.clone()).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: a teardown naming {body} once the enable is due expected 200, \
             got {} body {:?}",
            disabled.status, disabled.body
        );
        let identifier = disabled.string("operationIdentifier");
        assert!(
            is_operation_identifier(&identifier),
            "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {identifier:?}"
        );
        assert_ne!(
            &identifier, enable_operation,
            "case {case:?}: the teardown must mint its own identifier, got the enable's \
             {enable_operation:?}"
        );

        // The model gives `DisableControl` no `@idempotent` trait, and the teardown it
        // started is still changing the control, so a repeat is the same conflict any other
        // overlapping call gets rather than a second teardown.
        let repeated = disable(&router, body).await;
        assert_refused(case, &repeated, 409, "ConflictException");

        let record = control_operation(&router, &identifier).await;
        assert_eq!(
            (
                member(&record, "operationIdentifier"),
                member(&record, "operationType"),
                member(&record, "status"),
                member(&record, "controlIdentifier"),
                member(&record, "targetIdentifier"),
                member(&record, "enabledControlIdentifier"),
            ),
            (
                Some(identifier.as_str()),
                Some("DISABLE_CONTROL"),
                Some("IN_PROGRESS"),
                Some(control(row.control.name).as_str()),
                Some(row.target.as_str()),
                Some(arn.as_str()),
            ),
            "case {case:?}: expected the teardown's own record, IN_PROGRESS against the \
             control and target the enablement carries, got {record:?}"
        );
        assert!(
            record.get("startTime").is_some() && record.get("endTime").is_none(),
            "case {case:?}: expected a startTime and no endTime while the teardown is in \
             flight, got {record:?}"
        );
        assert!(
            record.get("statusMessage").is_none(),
            "case {case:?}: a statusMessage is reported in case of failure only, got {record:?}"
        );

        // The teardown is asynchronous, so the enablement is still a resource a reporting
        // call reports — `UNDER_CHANGE`, with the teardown as its last operation.
        let reported = details(&router, arn).await;
        assert_eq!(
            (status(&reported), last_operation(&reported)),
            (Some("UNDER_CHANGE"), Some(identifier.as_str())),
            "case {case:?}: expected the enablement reported UNDER_CHANGE by its teardown, \
             got {reported:?}"
        );

        teardowns.push((case, arn.clone(), identifier));
    }

    // One operation per call: no identifier is handed out twice.
    let mut minted: Vec<(&'static str, &'static str, &String)> = Vec::new();
    let all = enabled
        .iter()
        .map(|(case, _, identifier)| (*case, "enable", identifier))
        .chain(
            teardowns
                .iter()
                .map(|(case, _, identifier)| (*case, "disable", identifier)),
        );
    for (case, kind, identifier) in all {
        for (other_case, other_kind, other) in &minted {
            assert_ne!(
                identifier, *other,
                "case {case:?} ({kind}) and case {other_case:?} ({other_kind}): two calls \
                 must mint different operationIdentifiers, got {identifier:?} twice"
            );
        }
        minted.push((case, kind, identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (row, (case, arn, identifier)) in rows.iter().zip(&teardowns) {
        let record = control_operation(&router, identifier).await;
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("DISABLE_CONTROL"), Some("SUCCEEDED")),
            "case {case:?}: the teardown's record must outlive the resource as \
             DISABLE_CONTROL SUCCEEDED, got {record:?}"
        );
        let end_time = record.get("endTime").cloned();
        assert!(
            end_time.is_some(),
            "case {case:?}: expected an endTime once the teardown is due, got {record:?}"
        );
        let reread = control_operation(&router, identifier).await;
        assert_eq!(
            reread.get("endTime").cloned(),
            end_time,
            "case {case:?}: re-reading a terminal teardown must report the same endTime, got \
             {reread:?}"
        );

        // The enablement it tore down is gone: neither it nor its tags resolve.
        let gone = get_enabled_control(&router, arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a torn-down enablement must resolve no longer, got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
        let tagged = read_tags(&router, arn).await;
        assert_eq!(
            (tagged.status, tagged.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a torn-down enablement's tags must go with it, got {:?} body {:?}",
            (tagged.status, tagged.error_code.as_deref()),
            tagged.body
        );
        let again = disable(&router, naming(row.address, arn, row.control, &row.target)).await;
        assert_refused(case, &again, 404, "ResourceNotFoundException");
    }

    // Each pair enables again, as a NEW resource.
    for (row, (case, arn, _)) in rows.iter().zip(&teardowns) {
        let (reenabled, _) = enable_row(&router, row).await;
        assert_ne!(
            &reenabled, arn,
            "case {case:?}: a re-enabled pair must mint a new ARN, got the torn-down {arn:?}"
        );
    }
}

/// RULE: a request acts on the enablement it NAMES — by its own ARN, by the
/// control/target pair in either published ARN form, or by all three members naming one
/// enablement. A request naming neither form, or whose members name different enablements,
/// is refused with `ValidationException` (400); one naming an enablement this account does
/// not hold is `ResourceNotFoundException` (404); and every refusal tears nothing down.
#[tokio::test]
async fn a_request_acts_on_the_enablement_it_names() {
    let router = emulator();

    // A resource of ANOTHER kind that the account really holds, for the cross-store rows.
    let created = call(
        &router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.3", "manifest": simple_manifest() }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let landing_zone_arn = created.string("arn");

    // Two enablements of DIFFERENT controls on DIFFERENT targets, so a row can cross the
    // pair over and name neither.
    let first = plain_control(0);
    let second = plain_control(1);
    let first_target = ou("e001");
    let second_target = ou("e002");
    let (first_arn, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control(first.name),
            "targetIdentifier": first_target,
            "tags": { "owner": "platform" },
        }),
    )
    .await;
    let (second_arn, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control(second.name),
            "targetIdentifier": second_target,
        }),
    )
    .await;
    let resource_id = first_arn
        .rsplit('/')
        .next()
        .expect("the minted ARN carries a resource id")
        .to_string();

    // Past the settle span, so a refusal below cannot be an in-flight enable's 409.
    tokio::time::sleep(past_the_settle_span()).await;

    // Every row satisfies the members' @length 20..2048 and
    // @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$ — including both published length bounds — so
    // the front door passes them all and the HANDLER is what refuses them.
    let malformed: Vec<(&str, serde_json::Value)> = vec![
        ("no member at all, so no enablement is named", serde_json::json!({})),
        (
            "controlIdentifier alone, which names a control but no target",
            serde_json::json!({ "controlIdentifier": control(first.name) }),
        ),
        (
            "targetIdentifier alone, which names a target but no control",
            serde_json::json!({ "targetIdentifier": first_target }),
        ),
        (
            "an ARN with a controlIdentifier naming ANOTHER control",
            serde_json::json!({
                "enabledControlIdentifier": first_arn,
                "controlIdentifier": control(second.name),
            }),
        ),
        (
            "an ARN with a targetIdentifier naming ANOTHER target",
            serde_json::json!({
                "enabledControlIdentifier": first_arn,
                "targetIdentifier": second_target,
            }),
        ),
        (
            "an ARN with a controlIdentifier that is no control reference at all",
            serde_json::json!({
                "enabledControlIdentifier": first_arn,
                "controlIdentifier": first_target,
            }),
        ),
        (
            "a pair whose controlIdentifier is no control reference at all",
            serde_json::json!({
                "controlIdentifier": landing_zone_arn,
                "targetIdentifier": first_target,
            }),
        ),
        (
            "a pair whose controlIdentifier is an ARN at the length min of 20",
            serde_json::json!({
                "controlIdentifier": arn_of_length(20),
                "targetIdentifier": first_target,
            }),
        ),
        (
            "a pair whose controlIdentifier is an ARN at the length max of 2048",
            serde_json::json!({
                "controlIdentifier": arn_of_length(2048),
                "targetIdentifier": first_target,
            }),
        ),
    ];

    let missing: Vec<(&str, serde_json::Value)> = vec![
        (
            "an enabledControlIdentifier at exactly the length min of 20",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(20) }),
        ),
        (
            "an enabledControlIdentifier at exactly the length max of 2048",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(2048) }),
        ),
        (
            "an enabledcontrol ARN no enablement was minted under",
            serde_json::json!({
                "enabledControlIdentifier":
                    "arn:aws:controltower:us-west-2:123456789012:enabledcontrol/\
                     00000000-0000-4000-8000-000000000000"
            }),
        ),
        (
            "another service's ARN ending in this enablement's resource id",
            serde_json::json!({
                "enabledControlIdentifier": format!("arn:aws:iam::123456789012:role/{resource_id}")
            }),
        ),
        (
            "the control's own ARN rather than its enablement's",
            serde_json::json!({ "enabledControlIdentifier": control(first.name) }),
        ),
        (
            "the target's ARN rather than the enablement's",
            serde_json::json!({ "enabledControlIdentifier": first_target }),
        ),
        (
            "the ARN of a landing zone the account does hold",
            serde_json::json!({ "enabledControlIdentifier": landing_zone_arn }),
        ),
        (
            "a pair crossing the two enablements over, which is neither",
            serde_json::json!({
                "controlIdentifier": control(first.name),
                "targetIdentifier": second_target,
            }),
        ),
        (
            "a pair naming a catalogued control nothing enabled",
            serde_json::json!({
                "controlIdentifier": control(plain_control(2).name),
                "targetIdentifier": first_target,
            }),
        ),
        (
            "a pair naming a mandatory control, which this ungoverned target carries no \
             enablement of",
            serde_json::json!({
                "controlIdentifier": control(mandatory_control().name),
                "targetIdentifier": first_target,
            }),
        ),
        (
            "a pair naming a control ARN the catalog does not publish",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_NO_SUCH_CONTROL"),
                "targetIdentifier": first_target,
            }),
        ),
        (
            "a pair naming a catalog ARN the catalog does not publish",
            serde_json::json!({
                "controlIdentifier": catalog_arn("zzzzzzzzzzzzzzzzzzzzzzzzzz"),
                "targetIdentifier": first_target,
            }),
        ),
    ];

    for (case, body) in &malformed {
        let refused = disable(&router, body.clone()).await;
        assert_refused(case, &refused, 400, "ValidationException");
    }
    for (case, body) in &missing {
        let refused = disable(&router, body.clone()).await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
    }
    assert!(
        teardown_records(&router).await.is_empty(),
        "no refusal may append a DISABLE_CONTROL record, got {:?}",
        teardown_records(&router).await
    );

    // None of those refusals tore anything down: both enablements still resolve.
    for (case, arn) in [("the first", &first_arn), ("the second", &second_arn)] {
        let reported = details(&router, arn).await;
        assert_eq!(
            status(&reported),
            Some("SUCCEEDED"),
            "case {case:?}: an enablement no accepted call touched must still be SUCCEEDED, \
             got {reported:?}"
        );
    }

    // And every accepted addressing form tears its OWN enablement down. One enablement per
    // form, because the first accepted teardown consumes the one it names.
    let forms = [
        Address::Arn,
        Address::ControlPair,
        Address::CatalogPair,
        Address::ArnAndPair,
    ];
    for (index, address) in forms.into_iter().enumerate() {
        let control_entry = plain_control(index);
        let target = ou(&format!("f0{index}"));
        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control(control_entry.name),
                "targetIdentifier": target,
            }),
        )
        .await;
        tokio::time::sleep(past_the_settle_span()).await;

        let body = naming(address, &arn, control_entry, &target);
        let disabled = disable(&router, body.clone()).await;
        assert_eq!(
            disabled.status, 200,
            "case {address:?} naming {body}: expected 200, got {} body {:?}",
            disabled.status, disabled.body
        );
        let record = control_operation(&router, &disabled.string("operationIdentifier")).await;
        assert_eq!(
            (
                member(&record, "operationType"),
                member(&record, "enabledControlIdentifier")
            ),
            (Some("DISABLE_CONTROL"), Some(arn.as_str())),
            "case {address:?}: expected a DISABLE_CONTROL record against {arn:?}, got {record:?}"
        );
    }
}

/// RULE (the blast radius of a teardown): a `DisableControl` removes only the enablement its
/// request names. Every sibling stays enabled with its own tags and status — so its pair
/// cannot be enabled a second time — and every other operation record keeps its own type
/// and status.
#[tokio::test]
async fn a_teardown_removes_only_the_enablement_it_names() {
    let router = set_up_emulator().await;
    let rows = rows();
    assert!(
        rows.len() >= 3,
        "this case needs at least three enablements to tell siblings apart, got {}",
        rows.len()
    );

    let mut enabled: Vec<(&'static str, String, String)> = Vec::new();
    for row in &rows {
        let (arn, enable_operation) = enable_row(&router, row).await;
        enabled.push((row.case, arn, enable_operation));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // Tear down a MIDDLE row, so neither the first nor the last enablement is the one being
    // removed.
    let torn_index = rows.len() / 2;
    let torn_row = &rows[torn_index];
    let (torn_case, torn_arn, _) = enabled[torn_index].clone();
    let disabled = disable(
        &router,
        naming(torn_row.address, &torn_arn, torn_row.control, &torn_row.target),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "case {torn_case:?}: teardown expected 200, got {} body {:?}",
        disabled.status, disabled.body
    );
    let torn_operation = disabled.string("operationIdentifier");

    tokio::time::sleep(past_the_settle_span()).await;

    let gone = get_enabled_control(&router, &torn_arn).await;
    assert_eq!(
        (gone.status, gone.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {torn_case:?}: the torn-down ARN expected 404 ResourceNotFoundException, got \
         {:?} body {:?}",
        (gone.status, gone.error_code.as_deref()),
        gone.body
    );

    for (index, (case, arn, enable_operation)) in enabled.iter().enumerate() {
        if index == torn_index {
            continue;
        }

        // The sibling's enable record is untouched by another enablement's teardown.
        let record = control_operation(&router, enable_operation).await;
        assert_eq!(
            (member(&record, "operationType"), member(&record, "status")),
            (Some("ENABLE_CONTROL"), Some("SUCCEEDED")),
            "case {case:?}: a sibling's enable record must keep its own type and status, got \
             {record:?}"
        );

        // The sibling is still enabled, with the enable as its last operation...
        let reported = details(&router, arn).await;
        assert_eq!(
            (status(&reported), last_operation(&reported)),
            (Some("SUCCEEDED"), Some(enable_operation.as_str())),
            "case {case:?}: a sibling must keep its own status and last operation, got \
             {reported:?}"
        );

        // ...so its pair cannot be enabled a second time...
        let again = enable_row_refused(&router, &rows[index]).await;
        assert_eq!(
            (again.status, again.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: re-enabling a sibling's pair expected 400 ValidationException, got \
             {:?} body {:?}",
            (again.status, again.error_code.as_deref()),
            again.body
        );

        // ...and the tags it was enabled with are still its own.
        let tagged = read_tags(&router, arn).await;
        let expected_tags = enabled_with_tags(&rows[index]);
        assert_eq!(
            (tagged.status, reported_tags(&tagged)),
            (200, expected_tags.clone()),
            "case {case:?}: a sibling must keep the tags it was enabled with \
             ({expected_tags:?}), got {:?} body {:?}",
            tagged.status,
            tagged.body
        );

        // ...and its own request still tears it down in turn.
        let sibling_teardown = disable(
            &router,
            naming(rows[index].address, arn, rows[index].control, &rows[index].target),
        )
        .await;
        assert_eq!(
            sibling_teardown.status, 200,
            "case {case:?}: a sibling must still be disablable after another's teardown, got \
             {} body {:?}",
            sibling_teardown.status, sibling_teardown.body
        );
        assert_ne!(
            sibling_teardown.string("operationIdentifier"),
            torn_operation,
            "case {case:?}: a sibling's teardown must mint its own identifier, got \
             {torn_operation:?}"
        );
    }

    // The torn-down pair enables again, as a new resource.
    let (reenabled, _) = enable_row(&router, torn_row).await;
    assert_ne!(
        reenabled, torn_arn,
        "case {torn_case:?}: a re-enabled pair must mint a new ARN, got the torn-down \
         {torn_arn:?}"
    );
}

/// One row of the `controlIdentifier` classification table: a value, and the answer the
/// rule gives it.
struct IdentifierRow {
    case: &'static str,
    control_identifier: String,
    status: u16,
    code: &'static str,
}

/// A control ARN, in either published form, that the catalog publishes NO entry for: the
/// rows whose answer is the 404 of a reference to a control that does not exist. Each value
/// is checked against `state::CONTROL_CATALOG` — the catalog the handler resolves a
/// `controlIdentifier` against — rather than assumed unpublished, and the rows vary the ARN
/// form, the shape of the unpublished name and the published entry they are derived from.
fn unpublished_control_identifiers() -> Vec<IdentifierRow> {
    let published_name = plain_control(0).name;
    let published_id = plain_control(0).catalog_id;
    let rows = vec![
        IdentifierRow {
            case: "a control ARN whose name the catalog does not publish",
            control_identifier: control("AWS-GR_NO_SUCH_CONTROL"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        IdentifierRow {
            case: "a control ARN whose name is a published name with one character appended",
            control_identifier: control(&format!("{published_name}X")),
            status: 404,
            code: "ResourceNotFoundException",
        },
        IdentifierRow {
            case: "a control ARN whose name is a published name with its last character cut",
            control_identifier: control(&published_name[..published_name.len() - 1]),
            status: 404,
            code: "ResourceNotFoundException",
        },
        IdentifierRow {
            case: "a control catalog ARN whose id the catalog does not publish",
            control_identifier: catalog_arn("zzzzzzzzzzzzzzzzzzzzzzzzzz"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        IdentifierRow {
            case: "a control catalog ARN whose id is a published id with its first character \
                   rotated",
            control_identifier: catalog_arn(&format!("0{}", &published_id[1..])),
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];
    for row in &rows {
        let resource = row
            .control_identifier
            .rsplit('/')
            .next()
            .expect("a control ARN carries a resource segment");
        assert!(
            !CONTROL_CATALOG
                .iter()
                .any(|entry| entry.name == resource || entry.catalog_id == resource),
            "case {:?}: this row needs a control the catalog does NOT publish, but it \
             publishes {resource:?}",
            row.case
        );
    }
    rows
}

/// Values that are NOT a control reference at all: the rows whose answer is the 400 of a
/// malformed input. The forms come from `state::is_control_arn` — a control is named by
/// `arn:<partition>:controltower:<region>::control/<NAME>` or
/// `arn:<partition>:controlcatalog:::control/<id>`, neither naming an account — so a row
/// breaks one segment of a form at a time, including values whose RESOURCE segment is a
/// published control's.
fn non_control_references(target: &str, enabled_arn: &str) -> Vec<IdentifierRow> {
    let published = plain_control(0);
    vec![
        IdentifierRow {
            case: "an organizational unit's ARN, which names a target rather than a control",
            control_identifier: target.to_string(),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "the enablement's own ARN, which names an enablement rather than a control",
            control_identifier: enabled_arn.to_string(),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "another service's ARN",
            control_identifier: "arn:aws:iam::123456789012:role/ControlTowerAdmin".to_string(),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "a published control's name under an ARN that names an ACCOUNT, which no \
                   control ARN does",
            control_identifier: format!(
                "arn:aws:controltower:us-west-2:123456789012:control/{}",
                published.name
            ),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "a published control's name lowercased, which is no control name",
            control_identifier: control(&published.name.to_lowercase()),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "a published catalog id under a control catalog ARN naming a REGION, which \
                   that global catalog does not",
            control_identifier: format!(
                "arn:aws:controlcatalog:us-west-2::control/{}",
                published.catalog_id
            ),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "a control catalog ARN whose resource is not a 26-character catalog id",
            control_identifier: catalog_arn("tooshorttobeanid"),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "an ARN at exactly the member's length min of 20",
            control_identifier: arn_of_length(20),
            status: 400,
            code: "ValidationException",
        },
        IdentifierRow {
            case: "an ARN at exactly the member's length max of 2048",
            control_identifier: arn_of_length(2048),
            status: 400,
            code: "ValidationException",
        },
    ]
}

/// RULE: a `controlIdentifier` is classified by what IT names, whatever other members of
/// the request accompany it — a control ARN, in either form AWS publishes, that names no
/// catalogued control references a resource that does not exist (404
/// `ResourceNotFoundException`), while a value that is no control reference at all is a
/// malformed input (400 `ValidationException`). Only an identifier that DOES name a
/// catalogued control is then compared with the enablement the request names another way,
/// and disagreeing there is the mismatch `ValidationException` (400).
///
/// Asserted over the three addressing forms that carry the member (the control/target pair,
/// `enabledControlIdentifier` + `controlIdentifier`, and all three members) and over
/// `EnableControl`, which reads the same member: one value, one answer, down to the message.
#[tokio::test]
async fn a_control_identifier_is_classified_by_what_it_names() {
    let router = set_up_emulator().await;

    let entry = plain_control(0);
    let other = plain_control(1);
    let target = ou("g001");
    let (arn, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control(entry.name),
            "targetIdentifier": target,
        }),
    )
    .await;
    // Past the settle span, so no refusal below can be an in-flight enable's 409.
    tokio::time::sleep(past_the_settle_span()).await;

    let mut rows = unpublished_control_identifiers();
    rows.extend(non_control_references(&target, &arn));

    for row in &rows {
        let case = row.case;
        let identifier = &row.control_identifier;
        // The three ways a `DisableControl` request carries this member, plus the sibling
        // operation that reads it. Every one of them answers the value itself, so all four
        // answers must agree.
        let addressed: Vec<(&str, &str, serde_json::Value)> = vec![
            (
                "the control/target pair",
                "/disable-control",
                serde_json::json!({
                    "controlIdentifier": identifier,
                    "targetIdentifier": target,
                }),
            ),
            (
                "enabledControlIdentifier with controlIdentifier",
                "/disable-control",
                serde_json::json!({
                    "enabledControlIdentifier": arn,
                    "controlIdentifier": identifier,
                }),
            ),
            (
                "all three members",
                "/disable-control",
                serde_json::json!({
                    "enabledControlIdentifier": arn,
                    "controlIdentifier": identifier,
                    "targetIdentifier": target,
                }),
            ),
            (
                "EnableControl, which reads the same member",
                "/enable-control",
                serde_json::json!({
                    "controlIdentifier": identifier,
                    "targetIdentifier": ou("g002"),
                }),
            ),
        ];

        let mut answers: Vec<(&str, u16, Option<String>, String)> = Vec::new();
        for (form, path, body) in addressed {
            let refused = call(&router, path, body).await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (row.status, Some(row.code)),
                "case {case:?} addressed by {form}: expected {} {}, got {:?} body {:?}",
                row.status,
                row.code,
                (refused.status, refused.error_code.as_deref()),
                refused.body
            );
            let message = refused.string("message");
            assert!(
                message.contains(identifier.as_str()),
                "case {case:?} addressed by {form}: the refusal must name the identifier it \
                 refused, got {message:?}"
            );
            answers.push((form, refused.status, refused.error_code.clone(), message));
        }
        let (first_form, first_status, first_code, first_message) = answers[0].clone();
        for (form, status, code, message) in &answers[1..] {
            assert_eq!(
                (*status, code.as_deref(), message.as_str()),
                (first_status, first_code.as_deref(), first_message.as_str()),
                "case {case:?}: {form} must answer this identifier exactly as {first_form} \
                 does, got {:?}",
                (*status, code.as_deref(), message.as_str())
            );
        }
    }

    // An identifier that DOES name a catalogued control, but not this enablement's, is the
    // mismatch — in either published form, and whether or not the target is sent too.
    for (case, body) in [
        (
            "another catalogued control by its control ARN",
            serde_json::json!({
                "enabledControlIdentifier": arn,
                "controlIdentifier": control(other.name),
            }),
        ),
        (
            "another catalogued control by its control catalog ARN",
            serde_json::json!({
                "enabledControlIdentifier": arn,
                "controlIdentifier": catalog_arn(other.catalog_id),
                "targetIdentifier": target,
            }),
        ),
        (
            "a mandatory catalogued control, which this enablement is not of",
            serde_json::json!({
                "enabledControlIdentifier": arn,
                "controlIdentifier": control(mandatory_control().name),
            }),
        ),
    ] {
        let refused = disable(&router, body).await;
        assert_refused(case, &refused, 400, "ValidationException");
        let message = refused.string("message");
        assert!(
            message.contains(entry.name),
            "case {case:?}: a mismatch must report the control this enablement IS of, got \
             {message:?}"
        );
    }

    // Not one of those refusals tore anything down or started an operation.
    assert!(
        teardown_records(&router).await.is_empty(),
        "no refusal may append a DISABLE_CONTROL record, got {:?}",
        teardown_records(&router).await
    );
    let reported = details(&router, &arn).await;
    assert_eq!(
        status(&reported),
        Some("SUCCEEDED"),
        "the enablement no accepted call touched must still be SUCCEEDED, got {reported:?}"
    );

    // And an identifier that names the enablement's OWN control passes the cross-check, in
    // either published form.
    for (index, control_identifier) in [control(entry.name), catalog_arn(entry.catalog_id)]
        .into_iter()
        .enumerate()
    {
        let case = &control_identifier;
        let accepted_target = ou(&format!("g1{index}"));
        let (accepted_arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control(entry.name),
                "targetIdentifier": accepted_target,
            }),
        )
        .await;
        tokio::time::sleep(past_the_settle_span()).await;

        let disabled = disable(
            &router,
            serde_json::json!({
                "enabledControlIdentifier": accepted_arn,
                "controlIdentifier": control_identifier,
                "targetIdentifier": accepted_target,
            }),
        )
        .await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: an identifier naming this enablement's own control must be \
             accepted, got {} body {:?}",
            disabled.status, disabled.body
        );
        assert!(
            is_operation_identifier(&disabled.string("operationIdentifier")),
            "case {case:?}: an accepted teardown must return an operation identifier, got \
             {:?}",
            disabled.body
        );
    }
}

/// `EnableControl` for one row WITHOUT asserting it succeeded, for the cases whose subject
/// is that a pair is still enabled.
async fn enable_row_refused(router: &MultiServiceRouter, row: &Row) -> Answer {
    let mut body = serde_json::json!({
        "controlIdentifier": control(row.control.name),
        "targetIdentifier": row.target,
    });
    if let Some(parameters) = &row.parameters {
        body["parameters"] = parameters.clone();
    }
    if let Some(tags) = &row.tags {
        body["tags"] = tags.clone();
    }
    call(router, "/enable-control", body).await
}
