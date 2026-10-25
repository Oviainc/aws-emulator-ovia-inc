// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateVpcLink`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the VPC link
//! create/read helpers and the PATCH wrappers are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `vpc_links` and `patch_updates`.
//!
//! The RULE under test: `UpdateVpcLink` applies every entry of `patchOperations` to the
//! stored `VpcLink` record the `vpcLinkId` names, in request order, and reports the
//! patched record — which a later `GetVpcLink` reads back member for member — while a
//! patch the service cannot apply is refused with a modelled error and leaves the stored
//! record exactly as it was.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! of members, ops, starting records and list lengths; the paths and ops the resource does
//! not support; the character bound of each patchable member and one past it; the status
//! the clock owes across an update; repeated application of one list; and the forms an id
//! that names no stored link takes.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_STRING_CHARS, VPC_LINK_AVAILABLE, VPC_LINK_PENDING, VPC_LINK_PROVISION_SPAN,
};

use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, string_member, without, Answer, Gateway};
use crate::vpc_links::{create_vpc_link, get_vpc_link, target_arn};

/// Slack added to a stored span before asserting the state it is due in, so a loaded
/// machine does not read the record one tick early.
const MARGIN: Duration = Duration::from_millis(100);

/// One `patchOperations` entry, as a client spells it on the wire; `None` is an op that
/// carries no `value`.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// The `UpdateVpcLink`/`GetVpcLink` URI for one link of the account.
fn link_uri(vpc_link_id: &str) -> String {
    format!("/vpclinks/{vpc_link_id}")
}

/// `UpdateVpcLink` on `vpc_link_id`, answering whatever the service said.
async fn update(gateway: &Gateway, vpc_link_id: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &link_uri(vpc_link_id), operations).await
}

/// The baseline link every row that patches a named record starts from, unless the row
/// names its own: both patchable members at known values, so a replace has something to
/// move and a remove something to clear.
fn baseline_request() -> Value {
    json!({
        "name": "baseline-link",
        "description": "initial description",
        "targetArns": [target_arn(0)],
    })
}

/// Create a link from `request`, asserting the modelled 202, and answer its id.
async fn baseline(gateway: &Gateway, case: &str, request: Value) -> String {
    let created = create_vpc_link(gateway, request).await;
    assert_eq!(
        created.status, 202,
        "case {case}: expected 202 from CreateVpcLink, got {} with {:?}",
        created.status, created.body
    );
    string_member(&created.body, "id", case)
}

/// Assert every `(member, expected)` pair holds in `body`; a `Value::Null` expectation is
/// the member being ABSENT, which is what a cleared optional member looks like on the wire.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

#[tokio::test]
async fn a_patched_vpc_link_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names,
    // whether the record started with that member set, which op the row uses, how many ops
    // one request carries, whether two ops target one member, the form and size of the
    // value, and which non-patchable members the record carries beside them.
    type Row = (
        &'static str,
        Value,
        Vec<Value>,
        Vec<(&'static str, Value)>,
    );
    let rows: Vec<Row> = vec![
        (
            "replace the name",
            baseline_request(),
            vec![replace("/name", "renamed")],
            vec![
                ("name", json!("renamed")),
                ("description", json!("initial description")),
            ],
        ),
        (
            "replace the description",
            baseline_request(),
            vec![replace("/description", "a newer description")],
            vec![
                ("name", json!("baseline-link")),
                ("description", json!("a newer description")),
            ],
        ),
        (
            "add a description the link had none of",
            json!({ "name": "undescribed", "targetArns": [target_arn(0)] }),
            vec![op("add", "/description", Some("first description"))],
            vec![
                ("name", json!("undescribed")),
                ("description", json!("first description")),
            ],
        ),
        (
            "add on the name sets it just as replace does",
            baseline_request(),
            vec![op("add", "/name", Some("added-name"))],
            vec![
                ("name", json!("added-name")),
                ("description", json!("initial description")),
            ],
        ),
        (
            "remove the description of an optional member",
            baseline_request(),
            vec![op("remove", "/description", None)],
            vec![
                ("name", json!("baseline-link")),
                ("description", Value::Null),
            ],
        ),
        (
            "both patchable members in one request",
            baseline_request(),
            vec![
                replace("/name", "both"),
                replace("/description", "both described"),
            ],
            vec![
                ("name", json!("both")),
                ("description", json!("both described")),
            ],
        ),
        (
            "two ops on one member apply in request order",
            baseline_request(),
            vec![replace("/name", "first"), replace("/name", "second")],
            vec![
                ("name", json!("second")),
                ("description", json!("initial description")),
            ],
        ),
        (
            "a description cleared and then set again",
            baseline_request(),
            vec![
                op("remove", "/description", None),
                replace("/description", "set-again"),
            ],
            vec![("description", json!("set-again"))],
        ),
        (
            "a name outside ASCII",
            baseline_request(),
            vec![replace("/name", "ссылка-✓")],
            vec![("name", json!("ссылка-✓"))],
        ),
        (
            "a name at the character bound",
            baseline_request(),
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS))],
            vec![("name", json!("n".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "a description at the character bound",
            baseline_request(),
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS))],
            vec![("description", json!("d".repeat(MAX_STRING_CHARS)))],
        ),
        (
            "an empty list reports the record unchanged",
            baseline_request(),
            vec![],
            vec![
                ("name", json!("baseline-link")),
                ("description", json!("initial description")),
            ],
        ),
        (
            "a link with several targets keeps every one of them",
            json!({
                "name": "three-targets",
                "targetArns": [target_arn(0), target_arn(1), target_arn(2)],
            }),
            vec![replace("/name", "three-targets-renamed")],
            vec![
                ("name", json!("three-targets-renamed")),
                (
                    "targetArns",
                    json!([target_arn(0), target_arn(1), target_arn(2)]),
                ),
            ],
        ),
        (
            "a tagged link keeps its tags",
            json!({
                "name": "tagged",
                "targetArns": [target_arn(1)],
                "tags": { "team": "api", "env": "test" },
            }),
            vec![replace("/description", "tagged and described")],
            vec![
                ("description", json!("tagged and described")),
                ("tags", json!({ "team": "api", "env": "test" })),
            ],
        ),
    ];

    let gateway = Gateway::new();
    for (case, request, operations, expected) in rows {
        let id = baseline(&gateway, case, request).await;
        let updated = patch_ok(&gateway, case, &link_uri(&id), operations).await;
        assert_members(&updated, &expected, case, "UpdateVpcLink");
        assert_eq!(
            updated["id"],
            json!(id),
            "case {case}: the update must report the record's own id, got {:?}",
            updated["id"]
        );

        // The patched record is what is STORED, not just what the update answered.
        // `status` is left out of the comparison and asserted by the transition case
        // below, because it is the one member the CLOCK moves: the provisioning span may
        // pass between these two calls.
        let read = get_vpc_link(&gateway, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetVpcLink, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            without(&read.body, &["status"]),
            without(&updated, &["status"]),
            "case {case}: GetVpcLink must read back the patched record, got {:?} after {:?}",
            read.body,
            updated
        );
    }
}

#[tokio::test]
async fn an_update_reports_the_provisioning_status_the_clock_owes() {
    let gateway = Gateway::new();
    // The clock the assertions below are read against: a status measured while the call
    // demonstrably landed INSIDE the provisioning span must be `PENDING`, and one measured
    // after a machine slow enough to cross the span may already be `AVAILABLE`.
    let started = std::time::Instant::now();
    let id = baseline(&gateway, "the link to patch", baseline_request()).await;
    let patched = patch_ok(
        &gateway,
        "UpdateVpcLink within the provisioning span",
        &link_uri(&id),
        vec![replace("/name", "patched-while-pending")],
    )
    .await;
    let elapsed = started.elapsed();
    let owed: &[&str] = if elapsed < VPC_LINK_PROVISION_SPAN {
        &[VPC_LINK_PENDING]
    } else {
        &[VPC_LINK_PENDING, VPC_LINK_AVAILABLE]
    };
    let status = patched["status"].as_str();
    assert!(
        status.is_some_and(|status| owed.contains(&status)),
        "case UpdateVpcLink within the provisioning span: expected one of {owed:?} \
         {elapsed:?} after the create (span {VPC_LINK_PROVISION_SPAN:?}), got {:?}",
        patched["status"]
    );

    // Once the span has passed the link is available, and a patch neither restarts the
    // provisioning nor moves the status itself.
    tokio::time::sleep(VPC_LINK_PROVISION_SPAN + MARGIN).await;
    for round in 0..2 {
        let case = format!("UpdateVpcLink(round {round}) after the span");
        let patched = patch_ok(
            &gateway,
            &case,
            &link_uri(&id),
            vec![replace("/description", &format!("round {round}"))],
        )
        .await;
        assert_eq!(
            patched["status"].as_str(),
            Some(VPC_LINK_AVAILABLE),
            "case {case}: expected {VPC_LINK_AVAILABLE}, got {:?}",
            patched["status"]
        );
        let read = get_vpc_link(&gateway, &id).await;
        assert_eq!(
            read.body["status"].as_str(),
            Some(VPC_LINK_AVAILABLE),
            "case {case}: GetVpcLink must report {VPC_LINK_AVAILABLE}, got {:?}",
            read.body["status"]
        );
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    // Rows differ along the KIND of refusal: an op no single-valued member supports, an op
    // missing its value, a path naming a member the service assigns or that another
    // operation owns, a path that is not a JSON Pointer, a path reaching into a
    // single-valued member, a remove and an empty value on a member that always has one, a
    // value one character past the bound, and a list whose LAST op is refused (so the
    // earlier ops must not be kept either).
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("move is not supported", vec![op("move", "/name", None)]),
        (
            "copy is not supported",
            vec![json!({ "op": "copy", "path": "/name", "from": "/name" })],
        ),
        ("test is not supported", vec![op("test", "/name", None)]),
        ("a replace with no value", vec![op("replace", "/name", None)]),
        (
            "an add of the description with no value",
            vec![op("add", "/description", None)],
        ),
        ("an entry with no path", vec![json!({ "op": "replace" })]),
        ("an entry with no op", vec![json!({ "path": "/name" })]),
        (
            "the id is reported but minted by the service",
            vec![replace("/id", "zzzzzzzzzz")],
        ),
        (
            "the status is the clock's, not the caller's",
            vec![replace("/status", VPC_LINK_AVAILABLE)],
        ),
        (
            "the status message is not patchable either",
            vec![replace("/statusMessage", "all good")],
        ),
        (
            "the targets a link was provisioned against are not patchable",
            vec![replace("/targetArns", &target_arn(3))],
        ),
        (
            "nor is one entry of them added",
            vec![op(
                "add",
                "/targetArns/0",
                Some(&target_arn(3)),
            )],
        ),
        (
            "tags are changed through the tagging operations",
            vec![replace("/tags/team", "other")],
        ),
        (
            "a path naming no updateable member",
            vec![replace("/nosuchmember", "x")],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("name", "x")],
        ),
        ("an empty path", vec![replace("", "x")]),
        (
            "a path reaching into a single-valued member",
            vec![replace("/name/first", "x")],
        ),
        (
            "remove of a name a link always reports",
            vec![op("remove", "/name", None)],
        ),
        ("an empty name", vec![replace("/name", "")]),
        (
            "a name one character past the bound",
            vec![replace("/name", &"n".repeat(MAX_STRING_CHARS + 1))],
        ),
        (
            "a description one character past the bound",
            vec![replace("/description", &"d".repeat(MAX_STRING_CHARS + 1))],
        ),
        (
            "a list whose last op is refused",
            vec![
                replace("/name", "never stored"),
                replace("/nosuchmember", "x"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let baseline_members = vec![
        ("name", json!("baseline-link")),
        ("description", json!("initial description")),
        ("targetArns", json!([target_arn(0)])),
    ];
    // ONE stored link for every row: a refused patch must leave the record exactly as it
    // was, so each row both checks the refusal and re-checks the record the previous rows
    // must not have changed either.
    let id = baseline(&gateway, "the link every row patches", baseline_request()).await;

    for (case, operations) in &cases {
        let answer = update(&gateway, &id, operations.clone()).await;
        assert_failure(&answer, 400, "BadRequestException", case);

        // Nothing was applied: the stored record is the one the create left behind.
        let read = get_vpc_link(&gateway, &id).await;
        assert_members(&read.body, &baseline_members, case, "GetVpcLink");
    }

    // A path the resource has no updateable member for is refused with the fixed sentence
    // and the path the request sent, and nothing else.
    let answer = update(&gateway, &id, vec![replace("/id", "zzzzzzzzzz")]).await;
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, "Invalid patch path /id",
        "case the message of an unsupported path: expected the fixed sentence and the path, \
         got {message:?}"
    );
}

#[tokio::test]
async fn an_id_no_stored_link_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let stored = baseline(&gateway, "the stored link", baseline_request()).await;
    let operations = vec![replace("/name", "never applied")];

    // Rows differ along the form of the id: an id of the shape this service mints, a
    // shorter and a longer one, the link's NAME rather than its id, the stored id in
    // another case, and the stored id with one character stepped on.
    let mut rows: Vec<(String, String)> = vec![
        ("an unminted id".to_string(), "zzzzz99999".to_string()),
        ("a short id".to_string(), "abc".to_string()),
        (
            "a long id".to_string(),
            "abcdefghij0123456789".to_string(),
        ),
        (
            "the link's name, not its id".to_string(),
            "baseline-link".to_string(),
        ),
        (
            "the stored id upper-cased".to_string(),
            stored.to_uppercase(),
        ),
    ];
    let mut near_miss: Vec<char> = stored.chars().collect();
    if let Some(last) = near_miss.pop() {
        near_miss.push(if last == 'a' { 'b' } else { 'a' });
    }
    rows.push((
        "the stored id with one character changed".to_string(),
        near_miss.into_iter().collect(),
    ));

    for (case, id) in &rows {
        assert_failure(
            &update(&gateway, id, operations.clone()).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // None of those refusals touched the stored record.
    let read = get_vpc_link(&gateway, &stored).await;
    assert_members(
        &read.body,
        &[
            ("name", json!("baseline-link")),
            ("description", json!("initial description")),
        ],
        "after the refusals",
        "GetVpcLink",
    );
}

#[tokio::test]
async fn one_link_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let target = baseline(&gateway, "the link to patch", baseline_request()).await;
    let sibling = baseline(
        &gateway,
        "the sibling link",
        json!({
            "name": "sibling",
            "description": "the sibling's own description",
            "targetArns": [target_arn(1)],
        }),
    )
    .await;

    // Applying one list repeatedly lands on the same record: the patch sets members, it
    // does not accumulate.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateVpcLink(round {round})");
        seen.push(
            patch_ok(
                &gateway,
                &case,
                &link_uri(&target),
                vec![
                    replace("/name", "settled"),
                    replace("/description", "settled description"),
                ],
            )
            .await,
        );
    }
    assert_eq!(
        without(&seen[0], &["status"]),
        without(&seen[2], &["status"]),
        "case UpdateVpcLink repeated: the same list must land on the same record, got {:?} \
         then {:?}",
        seen[0],
        seen[2]
    );

    // The sibling is the record its own create left behind.
    let read = get_vpc_link(&gateway, &sibling).await;
    assert_members(
        &read.body,
        &[
            ("name", json!("sibling")),
            ("description", json!("the sibling's own description")),
            ("targetArns", json!([target_arn(1)])),
        ],
        "the sibling link",
        "GetVpcLink",
    );
    assert_eq!(
        read.body["id"],
        json!(sibling),
        "case the sibling link: expected its own id, got {:?}",
        read.body["id"]
    );
}
