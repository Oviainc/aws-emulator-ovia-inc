// SPDX-License-Identifier: CC0-1.0

//! Where a zero-ETL integration's `TargetArn` is RESOLVED, and the three answers
//! a target can give.
//!
//! The rule the tables assert: **`CreateIntegration`'s `TargetArn` names the data
//! warehouse namespace of EITHER Amazon Redshift control plane — the modelled
//! member admits an `arn:aws:redshift:…:namespace/…` and an
//! `arn:aws:redshift-serverless:…:namespace/…` alike
//! (`smithy_query(query="operation", operation="CreateIntegration")` -> TargetArn
//! @pattern `^arn:aws[a-z\-]*:redshift(-serverless)?:[a-z0-9\-]+:[0-9]{12}:(namespace\/|namespace:)[a-z0-9\-]+$`)
//! — so the ARN is resolved against the control plane that owns the namespace it
//! names and the outcome is classified three ways: a namespace that exists
//! nowhere is the permanent not-found class (404
//! `IntegrationTargetNotFoundFault`), a namespace that EXISTS while its data
//! warehouse is not active is the transient conflict class (400
//! `IntegrationConflictOperationFault`), and a namespace whose warehouse is
//! active takes the integration (200, `Status` `creating`).** A refused create
//! records nothing, and a second integration over one (`SourceArn`, `TargetArn`)
//! pair is refused with `IntegrationAlreadyExistsFault`.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, with the Redshift Serverless control plane
//! mounted beside Redshift on one engine exactly as the assembled emulator
//! mounts them — because a serverless namespace and the workgroup that is its
//! data warehouse are THAT service's resources, created here through its own
//! API.
//!
//! The rows differ along what the rule generalises over: which control plane owns
//! the namespace the ARN names, which of a namespace's two unique identifiers the
//! ARN carries (the id its own ARN is minted from, and the name it is addressed
//! by), which of the two resource separators the pattern admits is written, and
//! why a warehouse is not active (a workgroup still coming up, and a namespace
//! that has no workgroup at all).

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};

/// The Amazon S3 source every row replicates FROM unless it is about the pair
/// rule: the source is not what these tables vary.
const SOURCE: &str = "arn:aws:s3:::zetl-landing-zone";

/// The sentence Amazon Redshift words a target data warehouse that exists but is
/// not active with — entirely fixed text, reproduced here independently of the
/// handler so the test pins the wording rather than reading it back.
///
/// `pub(crate)` so the sibling suite about the ORDER the two ARNs are decided in
/// pins the same sentence rather than keeping a second copy of it.
pub(crate) const NOT_ACTIVE_MESSAGE: &str = "Unable to perform create operation on the specified \
                                  zero-ETL integration because its target Redshift data \
                                  warehouse is not active. Wait until the data warehouse is \
                                  active and try again.";

/// What a `TargetArn` resolves to, as the three refusal/acceptance classes the
/// rule names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// 404 `IntegrationTargetNotFoundFault`: no namespace of either control plane.
    Absent,
    /// 400 `IntegrationConflictOperationFault`: the namespace is there, the
    /// warehouse on it is not active.
    NotActive,
    /// 200: the warehouse is active and takes the integration.
    Active,
}

/// One row of the target table.
struct Row {
    case: &'static str,
    /// The name the create is made under, unique per row so the name rule never
    /// answers for the target rule.
    name: String,
    target: String,
    expected: Resolution,
}

/// Drive one awsQuery action with owned parameters.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// `CreateIntegration` with just the three required members.
async fn create_integration(
    emulator: &Emulator,
    name: &str,
    source: &str,
    target: &str,
) -> Answer {
    call(
        emulator,
        "CreateIntegration",
        &[
            ("IntegrationName".to_string(), name.to_string()),
            ("SourceArn".to_string(), source.to_string()),
            ("TargetArn".to_string(), target.to_string()),
        ],
    )
    .await
}

/// Create a serverless namespace through the control plane that owns it, and
/// answer the `namespaceArn` and `namespaceId` it minted.
///
/// `pub(crate)` so the sibling suite about the ORDER the two ARNs are decided in
/// seeds its targets through this one seam rather than a second copy of it.
pub(crate) async fn create_namespace(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = emulator
        .call_serverless("CreateNamespace", &serde_json::json!({ "namespaceName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case seed the namespace {name}: expected 200, got {status} with {body}"
    );
    let arn = body["namespace"]["namespaceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case seed the namespace {name}: expected a namespaceArn in {body}"))
        .to_string();
    let id = body["namespace"]["namespaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case seed the namespace {name}: expected a namespaceId in {body}"))
        .to_string();
    (arn, id)
}

/// Create a serverless workgroup on `namespace` through the control plane that
/// owns it. A new workgroup is reported `CREATING` and reaches `AVAILABLE` on its
/// own a moment later, which is what makes the transient class observable.
pub(crate) async fn create_workgroup(emulator: &Emulator, name: &str, namespace: &str) {
    let (status, body) = emulator
        .call_serverless(
            "CreateWorkgroup",
            &serde_json::json!({ "workgroupName": name, "namespaceName": namespace }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case seed the workgroup {name}: expected 200, got {status} with {body}"
    );
}

/// The status Amazon Redshift Serverless reports for one workgroup.
async fn workgroup_status(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call_serverless("GetWorkgroup", &serde_json::json!({ "workgroupName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case read the workgroup {name}: expected 200, got {status} with {body}"
    );
    body["workgroup"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wait until the workgroup the serverless control plane is bringing up reports
/// itself available, polling rather than assuming any one span.
pub(crate) async fn await_workgroup(emulator: &Emulator, name: &str) {
    for _ in 0..100 {
        if workgroup_status(emulator, name).await == "AVAILABLE" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case seed the workgroup {name}: expected it to become AVAILABLE");
}

/// The namespace ARN forms the modelled pattern admits for one namespace: the ARN
/// that service minted (which carries the `namespaceId`), the same ARN written
/// with the `namespace:` separator, and the namespace addressed by its NAME.
fn arn_forms(namespace_arn: &str, namespace_name: &str) -> (String, String, String) {
    (
        namespace_arn.to_string(),
        namespace_arn.replacen("namespace/", "namespace:", 1),
        format!(
            "arn:aws:redshift-serverless:{REGION}:{}:namespace/{namespace_name}",
            engine::emulator_account_id()
        ),
    )
}

/// Assert one row's create against the resolution the rule gives it.
async fn assert_row(emulator: &Emulator, row: &Row) {
    let answer = create_integration(emulator, &row.name, SOURCE, &row.target).await;
    match row.expected {
        Resolution::Absent => {
            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (404, Some("IntegrationTargetNotFoundFault")),
                "case {}: expected 404 IntegrationTargetNotFoundFault, got {} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(format!("Integration target not found: {}", row.target).as_str()),
                "case {}: expected the not-found message to name the ARN the request carried, got {:?}",
                row.case,
                answer.text("Message")
            );
        }
        Resolution::NotActive => {
            assert_eq!(
                (answer.status, answer.error_code().as_deref()),
                (400, Some("IntegrationConflictOperationFault")),
                "case {}: expected 400 IntegrationConflictOperationFault, got {} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(NOT_ACTIVE_MESSAGE),
                "case {}: expected the documented wait-for-the-warehouse message, got {:?}",
                row.case,
                answer.text("Message")
            );
        }
        Resolution::Active => {
            assert_eq!(
                (answer.status, answer.text("Status").as_deref()),
                (200, Some(state::INTEGRATION_STATUS_CREATING)),
                "case {}: expected 200 and the creating status, got {} / {:?} with body {}",
                row.case,
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                answer.text("TargetArn").as_deref(),
                Some(row.target.as_str()),
                "case {}: expected the target the request named back, got {:?}",
                row.case,
                answer.text("TargetArn")
            );
        }
    }
}

#[tokio::test]
async fn a_target_namespace_resolves_against_the_control_plane_that_owns_it() {
    let emulator = Emulator::with_serverless();

    // A serverless namespace whose data warehouse is still coming up, a second
    // one that has no warehouse at all, and a provisioned cluster's namespace
    // for the control rows.
    let (live_arn, live_id) = create_namespace(&emulator, "zetl-live").await;
    create_workgroup(&emulator, "zetl-live-wg", "zetl-live").await;
    let (bare_arn, _bare_id) = create_namespace(&emulator, "zetl-bare").await;
    let (live_by_id, live_colon, live_by_name) = arn_forms(&live_arn, "zetl-live");
    assert!(
        live_by_id.contains(&live_id),
        "case seed: expected the minted ARN to carry the namespace id, got {live_by_id} and {live_id}"
    );

    // PHASE 1: the warehouse is not active, so every form of a namespace that
    // EXISTS is the transient conflict class and every form of one that does not
    // is the permanent not-found class.
    let unknown_serverless = format!(
        "arn:aws:redshift-serverless:{REGION}:{}:namespace/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        engine::emulator_account_id()
    );
    let unknown_by_name = format!(
        "arn:aws:redshift-serverless:{REGION}:{}:namespace/zetl-nothing-created",
        engine::emulator_account_id()
    );
    let unknown_provisioned = format!(
        "arn:aws:redshift:{REGION}:{}:namespace/ffffffff-ffff-ffff-ffff-ffffffffffff",
        engine::emulator_account_id()
    );
    let coming_up: Vec<Row> = vec![
        Row {
            case: "the namespace by the ARN that service minted, warehouse still coming up",
            name: "zetlByIdEarly".to_string(),
            target: live_by_id.clone(),
            expected: Resolution::NotActive,
        },
        Row {
            case: "the same namespace with the namespace: separator the pattern admits",
            name: "zetlColonEarly".to_string(),
            target: live_colon.clone(),
            expected: Resolution::NotActive,
        },
        Row {
            case: "the same namespace addressed by the name it is known by",
            name: "zetlByNameEarly".to_string(),
            target: live_by_name.clone(),
            expected: Resolution::NotActive,
        },
        Row {
            case: "a namespace that holds no data warehouse at all",
            name: "zetlBareEarly".to_string(),
            target: bare_arn.clone(),
            expected: Resolution::NotActive,
        },
        Row {
            case: "a serverless namespace id nothing created",
            name: "zetlUnknownId".to_string(),
            target: unknown_serverless.clone(),
            expected: Resolution::Absent,
        },
        Row {
            case: "a serverless namespace name nothing created",
            name: "zetlUnknownName".to_string(),
            target: unknown_by_name.clone(),
            expected: Resolution::Absent,
        },
        Row {
            case: "a provisioned namespace ARN this account holds no cluster for",
            name: "zetlUnknownCluster".to_string(),
            target: unknown_provisioned.clone(),
            expected: Resolution::Absent,
        },
    ];
    for row in &coming_up {
        assert_row(&emulator, row).await;
    }

    // Every call above was REFUSED, so the account holds no integration: a
    // refused create records nothing.
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "IntegrationArn"),
        Vec::<String>::new(),
        "case a refused create records nothing: expected no integration, got body {}",
        listed.body
    );

    // PHASE 2: once the warehouse is active the identical creates succeed, while
    // the namespace with no warehouse and the ARNs naming nothing keep their own
    // answers.
    await_workgroup(&emulator, "zetl-live-wg").await;
    let settled: Vec<Row> = vec![
        Row {
            case: "the namespace by the ARN that service minted, warehouse active",
            name: "zetlByIdLive".to_string(),
            target: live_by_id.clone(),
            expected: Resolution::Active,
        },
        Row {
            case: "the namespace: separator form, warehouse active",
            name: "zetlColonLive".to_string(),
            target: live_colon.clone(),
            expected: Resolution::Active,
        },
        Row {
            case: "the name-addressed form, warehouse active",
            name: "zetlByNameLive".to_string(),
            target: live_by_name.clone(),
            expected: Resolution::Active,
        },
        Row {
            case: "a namespace that still holds no data warehouse",
            name: "zetlBareLate".to_string(),
            target: bare_arn.clone(),
            expected: Resolution::NotActive,
        },
        Row {
            case: "a serverless namespace id nothing created, after the wait",
            name: "zetlUnknownIdLate".to_string(),
            target: unknown_serverless,
            expected: Resolution::Absent,
        },
    ];
    for row in &settled {
        assert_row(&emulator, row).await;
    }

    // ROUND TRIP: the three integrations the accepted rows recorded are the three
    // every later read reports, and the INBOUND view of a serverless target is
    // the one its own ARN selects.
    let accepted: Vec<String> = settled
        .iter()
        .filter(|row| row.expected == Resolution::Active)
        .map(|row| state::integration_arn(&row.name, REGION))
        .collect();
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "IntegrationArn"),
        accepted,
        "case read the integrations back: expected the accepted rows in creation order, got body {}",
        listed.body
    );
    let inbound = call(
        &emulator,
        "DescribeInboundIntegrations",
        &[("TargetArn".to_string(), live_by_id.clone())],
    )
    .await;
    assert_eq!(
        all_text(&inbound.body, "IntegrationArn"),
        vec![accepted[0].clone()],
        "case inbound by a serverless target: expected the integration into it, got body {}",
        inbound.body
    );

    // A target the serverless control plane holds no namespace for is no
    // namespace for this read either.
    let inbound_unknown = call(
        &emulator,
        "DescribeInboundIntegrations",
        &[("TargetArn".to_string(), unknown_by_name.clone())],
    )
    .await;
    assert_eq!(
        (
            inbound_unknown.status,
            inbound_unknown.error_code().as_deref()
        ),
        (400, Some("InvalidNamespaceFault")),
        "case inbound by a serverless target nothing created: expected 400 InvalidNamespaceFault, got {} / {:?}",
        inbound_unknown.status,
        inbound_unknown.error_code()
    );
}

#[tokio::test]
async fn one_source_and_target_pair_is_one_integration() {
    let emulator = Emulator::with_serverless();
    let (namespace_arn, _id) = create_namespace(&emulator, "zetl-pair").await;
    create_workgroup(&emulator, "zetl-pair-wg", "zetl-pair").await;
    await_workgroup(&emulator, "zetl-pair-wg").await;

    let created = create_integration(&emulator, "zetlPairFirst", SOURCE, &namespace_arn).await;
    assert_eq!(
        created.status, 200,
        "case seed the integration: expected 200, got {} with body {}",
        created.status, created.body
    );

    // The PAIR is what an integration is: a second one over the same source and
    // the same target is refused however it is named.
    for (case, name) in [
        ("the same pair under another name", "zetlPairSecond"),
        ("the same pair under the same name", "zetlPairFirst"),
    ] {
        let duplicate = create_integration(&emulator, name, SOURCE, &namespace_arn).await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_deref()),
            (400, Some("IntegrationAlreadyExistsFault")),
            "case {case}: expected 400 IntegrationAlreadyExistsFault, got {} / {:?} with body {}",
            duplicate.status,
            duplicate.error_code(),
            duplicate.body
        );
        assert_eq!(
            duplicate.text("Message").as_deref(),
            Some(
                format!(
                    "An integration from source {SOURCE} to target {namespace_arn} already exists. \
                     Only one integration is allowed."
                )
                .as_str()
            ),
            "case {case}: expected the message to name the source and the target, got {:?}",
            duplicate.text("Message")
        );
    }

    // Another source into the same target is another integration, so the pair
    // rule is about the pair and not about the target alone.
    let other_source = create_integration(
        &emulator,
        "zetlPairOtherSource",
        "arn:aws:dynamodb:us-west-2:000000000000:table/orders",
        &namespace_arn,
    )
    .await;
    assert_eq!(
        other_source.status, 200,
        "case another source into the same target: expected 200, got {} with body {}",
        other_source.status, other_source.body
    );
    let listed = emulator.call("DescribeIntegrations", &[]).await;
    assert_eq!(
        all_text(&listed.body, "IntegrationName"),
        vec![
            "zetlPairFirst".to_string(),
            "zetlPairOtherSource".to_string()
        ],
        "case read the pair cases back: expected the two accepted integrations, got body {}",
        listed.body
    );
}
