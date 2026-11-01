// SPDX-License-Identifier: CC0-1.0

//! What an instance whose CREATION has not finished REPORTS, end to end over the
//! `awsJson1_1` wire.
//!
//! The rule under test, from `DescribeInstance`'s own documentation of the status
//! (`smithy_query(query="doc",shape="DescribeInstance")`): "CREATE_IN_PROGRESS - The
//! instance is in the process of being created. … While the instance is in the
//! CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance
//! operations." The sentence names TWO operations, not "every read": so while a
//! `CreateInstance` is still in flight, every operation that resolves that instance — or a
//! resource held under it — is refused, whether it writes or only reports, and the refusal
//! carries `state::instance_creation_in_progress_message` on the channel the calling
//! operation's own model declares. `instance_create_in_progress_writes.rs` pins the writes,
//! which answer it on their modelled `ConflictException`; this suite pins the instance-
//! scoped REPORTS, whose models declare no `ConflictException` and which therefore answer
//! it on their modelled `AccessDeniedException` (HTTP 403) — the channel each of them
//! already answers an instance this account does not hold on.
//!
//! The table spans the class the rule generalises over rather than one operation: every
//! instance-scoped report this window can reach — the tag report (with and without the
//! optional `InstanceArn`), the two Region reports, the two enumerations of what is held
//! under an instance, and the principal's application assignments — with rows that differ
//! in the page size they ask for, in the principal they name and in whether the request is
//! served for a reason of its own once the instance is active, so the refusal in the window
//! cannot be confused with a refusal of the request itself.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints, the
//! `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than bypassed.
//! Where a phase has to be on the far side of the activation instant it moves the STORED
//! instant rather than sleeping — the transition itself is still applied by the service's
//! own `materialize_due`, exactly as in production. The harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the fixture the one
//! `instance_create_in_progress_writes.rs` owns.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_create_in_progress_writes::{creating_instance, SEED_TAG_KEY, SEED_TAG_VALUE};
use crate::ssoadmin_permission_sets::{call, error_code, TEST_REGION};

/// A Region the created instance is NOT enabled in: `RegionName`'s pattern
/// `^([a-z]+-){2,3}\d$`, and not the [`TEST_REGION`] every call is signed for, which is the
/// Region the instance was created in and therefore its primary one.
const UNENABLED_REGION: &str = "eu-west-1";

/// The largest page a `MaxResults` member admits (`range 1..100`, from
/// `smithy_query(query="operation",operation="ListRegions")`), so one row asks for a page
/// size at the modelled bound rather than leaving it absent.
const MAX_RESULTS_CEILING: u64 = 100;

/// A `PrincipalId` in the bare-GUID form the model's pattern admits
/// (`^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`,
/// from `smithy_query(query="operation",operation="ListApplicationAssignmentsForPrincipal")`).
const PRINCIPAL_GUID: &str = "0f1e2d3c-4b5a-4697-8869-7a6b5c4d3e2f";

/// The same member in its `[0-9a-f]{10}-` prefixed form, so the two principal rows differ in
/// the form of the id as well as in the `PrincipalType` they name.
const PRINCIPAL_PREFIXED_GUID: &str = "13579bdf02-fedc1234-5678-4abc-8def-0123456789ab";

/// One row of the table: a REPORT made against an instance whose creation has not finished.
struct Row {
    label: &'static str,
    operation: &'static str,
    /// The request body, built from the instance's ARN.
    body: fn(&str) -> Value,
    /// What the SAME request must answer once the activation has fallen due: `None` for a
    /// served call, otherwise the `(http status, wire error code)` of the refusal this
    /// request earns for a reason of its own.
    once_active: Option<(u16, &'static str)>,
}

/// The rows the rule is asserted over.
const ROWS: &[Row] = &[
    Row {
        label: "ListTagsForResource on the instance itself",
        operation: "ListTagsForResource",
        body: |arn| json!({"ResourceArn": arn}),
        once_active: None,
    },
    Row {
        label: "ListTagsForResource scoped to the holding instance",
        operation: "ListTagsForResource",
        body: |arn| json!({"InstanceArn": arn, "ResourceArn": arn}),
        once_active: None,
    },
    Row {
        label: "DescribeRegion of the Region the instance was created in",
        operation: "DescribeRegion",
        body: |arn| json!({"InstanceArn": arn, "RegionName": TEST_REGION}),
        once_active: None,
    },
    Row {
        label: "DescribeRegion of a Region the instance is not enabled in",
        operation: "DescribeRegion",
        body: |arn| json!({"InstanceArn": arn, "RegionName": UNENABLED_REGION}),
        // Once the instance is active the describe is refused for its OWN reason: the
        // instance holds no record for that Region (`region_describe.rs`).
        once_active: Some((400, "ResourceNotFoundException")),
    },
    Row {
        label: "ListRegions with no page size",
        operation: "ListRegions",
        body: |arn| json!({"InstanceArn": arn}),
        once_active: None,
    },
    Row {
        label: "ListRegions asking for the largest page the model admits",
        operation: "ListRegions",
        body: |arn| json!({"InstanceArn": arn, "MaxResults": MAX_RESULTS_CEILING}),
        once_active: None,
    },
    Row {
        label: "ListApplications under the instance",
        operation: "ListApplications",
        body: |arn| json!({"InstanceArn": arn}),
        once_active: None,
    },
    Row {
        label: "ListTrustedTokenIssuers under the instance",
        operation: "ListTrustedTokenIssuers",
        body: |arn| json!({"InstanceArn": arn, "MaxResults": MAX_RESULTS_CEILING}),
        once_active: None,
    },
    Row {
        label: "ListApplicationAssignmentsForPrincipal for a USER principal",
        operation: "ListApplicationAssignmentsForPrincipal",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "PrincipalId": PRINCIPAL_GUID,
                "PrincipalType": "USER",
            })
        },
        once_active: None,
    },
    Row {
        label: "ListApplicationAssignmentsForPrincipal for a GROUP principal",
        operation: "ListApplicationAssignmentsForPrincipal",
        body: |arn| {
            json!({
                "InstanceArn": arn,
                "PrincipalId": PRINCIPAL_PREFIXED_GUID,
                "PrincipalType": "GROUP",
            })
        },
        once_active: None,
    },
];

/// The detail text an error answer carries, under either spelling of the member.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `DescribeInstance` for `arn`, as a 200 body — one of the two operations the sentence
/// names, so it is served throughout.
async fn describe(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200 for {arn}, got {status} {body}"
    );
    body
}

/// THE RULE: every instance-scoped report made while the instance's creation is in flight is
/// refused 403 `AccessDeniedException` carrying the window's own message, and is answered on
/// its own terms once the activation has fallen due.
#[tokio::test]
async fn an_instance_scoped_report_is_refused_while_the_instance_is_being_created() {
    for row in ROWS {
        let case = row.label;
        // Each row gets its own account: an account holds ONE instance, and a row must be
        // judged against an instance nothing else has touched.
        let (router, engine, instance_arn) = creating_instance().await;
        let request = (row.body)(&instance_arn);
        let expected = state::instance_creation_in_progress_message(&instance_arn);

        // The window is real: the instance reports the transient status throughout.
        let before = describe(&router, case, &instance_arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(state::STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: the fixture must be in the creation window, got {:?}",
            before["Status"]
        );

        let (status, refused) = call(&router, row.operation, &request).await;
        assert_eq!(
            status, 403,
            "case {case:?}: {} must be refused while the creation is in flight, got \
             {status} {refused}",
            row.operation
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {refused}"
        );
        assert_eq!(
            message_of(&refused),
            expected,
            "case {case:?}: expected the message {expected:?}, got {refused}"
        );

        // A refused report changed nothing a later caller sees: the instance's own describe
        // answers exactly what it answered before.
        let after = describe(&router, case, &instance_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused report must change nothing a later caller sees, got \
             {after} vs {before}"
        );

        // Once the clock makes the activation due, the SAME request is answered on its own
        // terms: served, or refused for a reason that is not the instance's status.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(60));
        let active = describe(&router, case, &instance_arn).await;
        assert_eq!(
            active["Status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the activation was due, got {:?}",
            active["Status"]
        );
        let (status, answer) = call(&router, row.operation, &request).await;
        match row.once_active {
            None => assert_eq!(
                status, 200,
                "case {case:?}: {} is served once the instance is active, got {status} \
                 {answer}",
                row.operation
            ),
            Some((expected_status, expected_code)) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: {} is refused for its own reason once the instance is \
                     active, got {status} {answer}",
                    row.operation
                );
                assert_eq!(
                    error_code(&answer),
                    expected_code,
                    "case {case:?}: expected {expected_code} once the instance is active, \
                     got {answer}"
                );
            }
        }
    }
}

/// The tag set the window withholds is the one the create stored: the report is refused
/// while the creation is in flight and, once the activation falls due, names exactly the
/// tags `CreateInstance` was given — so the refusal withheld a report rather than changing
/// what there was to report.
#[tokio::test]
async fn the_withheld_tag_report_is_the_one_the_create_stored() {
    let case = "the instance's own tag set";
    let (router, engine, instance_arn) = creating_instance().await;
    let request = json!({"ResourceArn": instance_arn});

    let (status, refused) = call(&router, "ListTagsForResource", &request).await;
    assert_eq!(
        status, 403,
        "case {case:?}: the report must be refused in the window, got {status} {refused}"
    );

    hold_activation(&engine, SystemTime::now() - Duration::from_secs(60));
    let (status, listed) = call(&router, "ListTagsForResource", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the report must be served once the instance is active, got \
         {status} {listed}"
    );
    assert_eq!(
        listed["Tags"],
        json!([{"Key": SEED_TAG_KEY, "Value": SEED_TAG_VALUE}]),
        "case {case:?}: expected the tag set the create stored, got {listed}"
    );
}

/// The other half of the same sentence: the two operations it NAMES are served inside the
/// window, and so is the enumeration that resolves no instance at all — `ListInstances`
/// takes no `InstanceArn`, and reporting the instance with its transient status is how a
/// caller sees the window it is in. So the refusals above are about which operation was
/// called, not about the instance being unreachable.
#[tokio::test]
async fn the_calls_the_window_serves_are_served_while_the_instance_is_being_created() {
    let case = "inside the creation window";
    let (router, _engine, instance_arn) = creating_instance().await;

    // The describe, repeatedly: it reports the transient status and, being a read, leaves
    // the record where it was.
    let first = describe(&router, case, &instance_arn).await;
    assert_eq!(
        first["Status"].as_str(),
        Some(state::STATUS_CREATE_IN_PROGRESS),
        "case {case:?}: the describe must report the transient status, got {first}"
    );
    let second = describe(&router, case, &instance_arn).await;
    assert_eq!(
        second, first,
        "case {case:?}: a repeated describe must answer identically, got {second} vs {first}"
    );

    // `ListInstances` resolves no `InstanceArn`, so the window does not withhold it: the
    // instance is listed, in the status it is in.
    let (status, listed) = call(&router, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInstances expected 200, got {status} {listed}"
    );
    let entry = listed["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Instances in {listed}"))
        .iter()
        .find(|instance| instance["InstanceArn"].as_str() == Some(instance_arn.as_str()))
        .unwrap_or_else(|| panic!("case {case:?}: {instance_arn} is not listed in {listed}"))
        .clone();
    assert_eq!(
        entry["Status"].as_str(),
        Some(state::STATUS_CREATE_IN_PROGRESS),
        "case {case:?}: the listed instance must carry the transient status, got {entry}"
    );

    // The delete, on an instance of its own: it is served inside the window.
    let (router, _engine, instance_arn) = creating_instance().await;
    let (status, deleted) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteInstance must be served inside the window, got {status} {deleted}"
    );
}
