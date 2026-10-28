// SPDX-License-Identifier: CC0-1.0

//! The per-type size, IOPS and throughput rules, driven through BOTH operations that
//! apply them — `CreateVolume` and `ModifyVolume` — over the wire this service really
//! speaks (awsJson1_0, `POST /`, `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs:
//!
//! > whether a configuration is one a volume type accepts — the size inside the range
//! > the type publishes, provisioned IOPS and a throughput inside theirs, and a
//! > performance member the type takes no value for omitted — is ONE rule of the type,
//! > not of the call: the configuration a create is refused for is the configuration a
//! > modification is refused for, with the same status, the same modelled code, the
//! > same named member and the same wording, and a configuration the type accepts is
//! > accepted by both.
//!
//! The rows differ along what that rule generalises over: the volume type (every one
//! the service publishes, read off its own catalog), the member (`SizeInGB`, `Iops`,
//! `Throughput`), which side of a published bound the value falls on (each bound and
//! the value one past it), and whether the member is out of range or not taken by the
//! type at all.
//!
//! Nothing here can pass against a canned answer: every modified id comes back out of
//! `CreateVolume`, each pair of refusals is compared field by field rather than against
//! text written here, and every acceptance is read back as the target the service
//! reports.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use wsinst::state::VolumeTypeSpec;

use crate::volume_lifecycle::create_volume;
use crate::workspace_instance_lifecycle::{call, error_code, harness, DEFAULT_TEST_REGION};

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// What a refusal of the per-type rule says, as a client reads it: the status, the
/// modelled error code, the members `FieldList` names and the wording. Compared whole,
/// so the two operations cannot answer the same bad configuration differently in any
/// of them.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    code: String,
    members: Vec<String>,
    message: String,
}

/// Read a [`Refusal`] out of one answer, failing the case when the answer is not a
/// refusal of the shape this service's validation refusals have.
fn refusal(case: &str, operation: &str, status: u16, payload: &Value) -> Refusal {
    let members = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no FieldList in {payload}"))
        .iter()
        .map(|field| {
            field["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Name in {payload}"))
                .to_string()
        })
        .collect();
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Message in {payload}"))
        .to_string();
    assert_eq!(
        payload["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?} on {operation}: expected the field-validation reason, got {payload}"
    );
    Refusal {
        status,
        code: error_code(payload),
        members,
        message,
    }
}

/// The smallest create body `spec` accepts in `zone`: the type's own floor size, with
/// provisioned IOPS only where the type requires them.
fn accepted_body(spec: &VolumeTypeSpec, zone: &str) -> Value {
    let mut body = json!({
        "AvailabilityZone": zone,
        "VolumeType": spec.volume_type,
        "SizeInGB": spec.size_range().0,
    });
    if spec.iops_required {
        body["Iops"] = json!(
            spec.iops
                .expect("a type that requires IOPS publishes a range")
                .0
        );
    }
    body
}

/// One configuration the type does not accept, reached through each of the two
/// operations that judge it.
struct SharedRow {
    case: String,
    /// The member both refusals must name.
    member: &'static str,
    /// The `CreateVolume` body whose configuration is the bad one.
    create: Value,
    /// The volume the modification starts from — a configuration the service accepts.
    modify_from: Value,
    /// The modification whose TARGET is the same bad configuration.
    modify: Value,
}

/// The rows the shared rule is asserted over, generated from the service's own
/// published catalog (`wsinst::state::VOLUME_TYPE_SPECS`) so no type the modelled
/// `VolumeTypeEnum` admits can be left out, and so a bound that changes in the catalog
/// changes the rows with it.
fn shared_rows(zone: &str) -> Vec<SharedRow> {
    let (modelled_min, modelled_max) = wsinst::state::MODELLED_VOLUME_SIZE_RANGE;
    // The type a create that names none provisions, whose floor is the lowest the
    // catalog publishes: it is the type a size-floor row starts a modification from.
    let default = wsinst::state::volume_type_spec(wsinst::state::DEFAULT_VOLUME_TYPE)
        .expect("the default volume type is in the catalog");
    let mut rows = Vec::new();

    for spec in &wsinst::state::VOLUME_TYPE_SPECS {
        let volume_type = spec.volume_type;
        let (size_min, size_max) = spec.size_range();

        // One past the ceiling the type publishes, where that value is still one the
        // modelled `SizeInGB` range carries — a value past the modelled range is
        // refused at the boundary instead, before either handler judges it.
        if size_max < modelled_max {
            let mut create = accepted_body(spec, zone);
            create["SizeInGB"] = json!(size_max + 1);
            rows.push(SharedRow {
                case: format!("{volume_type} one above the ceiling it publishes"),
                member: "SizeInGB",
                create,
                modify_from: accepted_body(spec, zone),
                modify: json!({ "SizeInGB": size_max + 1 }),
            });
        }

        // One below the floor the type publishes. A modification reaches that
        // configuration through a change of TYPE: a volume of the default type, whose
        // floor is lower, keeps its size and so lands below the target type's floor.
        let below_floor = size_min - 1;
        if below_floor >= modelled_min
            && below_floor >= default.size_range().0
            && below_floor <= default.size_range().1
        {
            let mut create = accepted_body(spec, zone);
            create["SizeInGB"] = json!(below_floor);
            rows.push(SharedRow {
                case: format!("{volume_type} one below the floor it publishes"),
                member: "SizeInGB",
                create,
                modify_from: json!({ "AvailabilityZone": zone, "SizeInGB": below_floor }),
                modify: json!({ "VolumeType": volume_type }),
            });
        }

        // `Iops`: outside the range a type that provisions them publishes, or named at
        // all for a type that provisions none. `0`, which the modelled `range 0..`
        // admits, is below every published floor.
        let bad_iops: Vec<(String, i32)> = match spec.iops {
            Some((min, max)) => vec![
                (format!("{volume_type} one below the Iops floor it publishes"), min - 1),
                (format!("{volume_type} one above the Iops ceiling it publishes"), max + 1),
                (format!("{volume_type} with Iops 0, below every published floor"), 0),
            ],
            None => vec![
                (format!("{volume_type} with Iops 0, which it does not provision"), 0),
                (
                    format!("{volume_type} with Iops 3000, which it does not provision"),
                    3_000,
                ),
            ],
        };
        for (case, iops) in bad_iops {
            let mut create = accepted_body(spec, zone);
            create["Iops"] = json!(iops);
            rows.push(SharedRow {
                case,
                member: "Iops",
                create,
                modify_from: accepted_body(spec, zone),
                modify: json!({ "Iops": iops }),
            });
        }

        // `Throughput`, the same two shapes: outside the published range, or named for
        // a type whose throughput is not configurable.
        let bad_throughput: Vec<(String, i32)> = match spec.throughput_mib {
            Some((min, max)) => vec![
                (
                    format!("{volume_type} one below the Throughput floor it publishes"),
                    min - 1,
                ),
                (
                    format!("{volume_type} one above the Throughput ceiling it publishes"),
                    max + 1,
                ),
            ],
            None => vec![
                (
                    format!("{volume_type} with Throughput 0, which it does not take"),
                    0,
                ),
                (
                    format!("{volume_type} with Throughput 125, which it does not take"),
                    125,
                ),
            ],
        };
        for (case, throughput) in bad_throughput {
            let mut create = accepted_body(spec, zone);
            create["Throughput"] = json!(throughput);
            rows.push(SharedRow {
                case,
                member: "Throughput",
                create,
                modify_from: accepted_body(spec, zone),
                modify: json!({ "Throughput": throughput }),
            });
        }
    }

    rows
}

#[tokio::test]
async fn one_rule_refuses_the_same_configuration_on_create_and_on_modify() {
    let (router, _engine) = harness();
    let zone = format!("{DEFAULT_TEST_REGION}a");
    let rows = shared_rows(&zone);

    // Every published type takes part, so no row set can pass while a type's rules go
    // through only one of the two operations.
    for volume_type in wsinst::ops::types::VOLUME_TYPE_ENUM_VALUES {
        assert!(
            rows.iter()
                .any(|row| row.case.starts_with(&format!("{volume_type} "))),
            "case catalog: {volume_type:?} takes part in no row, so its rules are judged \
             through one operation only"
        );
    }

    for row in &rows {
        let case = row.case.as_str();

        // The create path: the configuration is the body's own.
        let (create_status, create_payload) =
            call(&router, "CreateVolume", row.create.clone()).await;
        let created = refusal(case, "CreateVolume", create_status, &create_payload);

        // The modify path: the same configuration, reached as the TARGET of a
        // modification of a volume the service did accept.
        let id = create_volume(&router, row.modify_from.clone()).await;
        let mut request = row.modify.clone();
        request["VolumeId"] = json!(&id);
        let (modify_status, modify_payload) = call(&router, "ModifyVolume", request).await;
        let modified = refusal(case, "ModifyVolume", modify_status, &modify_payload);

        assert_eq!(
            (created.status, created.code.as_str(), &created.members),
            (
                400,
                "ValidationException",
                &vec![row.member.to_string()]
            ),
            "case {case:?}: expected the modelled validation refusal naming {:?}, got \
             {create_payload}",
            row.member
        );
        assert_eq!(
            modified, created,
            "case {case:?}: ModifyVolume must refuse the configuration exactly as \
             CreateVolume does, got {modify_payload:?} against {create_payload:?}"
        );
        assert!(
            created.message.contains(row.member),
            "case {case:?}: expected the wording to name {:?}, got {:?}",
            row.member,
            created.message
        );
        assert!(
            created.message.contains(
                row.create["VolumeType"]
                    .as_str()
                    .unwrap_or(wsinst::state::DEFAULT_VOLUME_TYPE)
            ),
            "case {case:?}: expected the wording to name the volume type it is read for, \
             got {:?}",
            created.message
        );
    }
}

#[tokio::test]
async fn one_rule_accepts_the_published_bounds_on_create_and_on_modify() {
    let (router, _engine) = harness();
    let zone = format!("{DEFAULT_TEST_REGION}a");

    for spec in &wsinst::state::VOLUME_TYPE_SPECS {
        let volume_type = spec.volume_type;
        let (size_min, size_max) = spec.size_range();

        // Each bound of each member the type takes, as a (member, value, target
        // member of the modification's answer) row.
        let mut bounds: Vec<(String, &str, i32, &str)> = vec![
            (
                format!("{volume_type} at the floor size it publishes"),
                "SizeInGB",
                size_min,
                "TargetSize",
            ),
            (
                format!("{volume_type} at the ceiling size it publishes"),
                "SizeInGB",
                size_max,
                "TargetSize",
            ),
        ];
        if let Some((min, max)) = spec.iops {
            bounds.push((
                format!("{volume_type} at the Iops floor it publishes"),
                "Iops",
                min,
                "TargetIops",
            ));
            bounds.push((
                format!("{volume_type} at the Iops ceiling it publishes"),
                "Iops",
                max,
                "TargetIops",
            ));
        }
        if let Some((min, max)) = spec.throughput_mib {
            bounds.push((
                format!("{volume_type} at the Throughput floor it publishes"),
                "Throughput",
                min,
                "TargetThroughput",
            ));
            bounds.push((
                format!("{volume_type} at the Throughput ceiling it publishes"),
                "Throughput",
                max,
                "TargetThroughput",
            ));
        }

        for (case, member, value, target_member) in bounds {
            // The create path provisions the bound outright.
            let mut body = accepted_body(spec, &zone);
            body[member] = json!(value);
            let (status, payload) = call(&router, "CreateVolume", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateVolume must accept a published bound, got {status} \
                 with {payload}"
            );

            // The modify path targets the same bound on a volume of the same type, and
            // reports it back as the target.
            let id = create_volume(&router, accepted_body(spec, &zone)).await;
            let (status, payload) = call(
                &router,
                "ModifyVolume",
                json!({ "VolumeId": &id, member: value }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: ModifyVolume must accept the same published bound, got \
                 {status} with {payload}"
            );
            assert_eq!(
                payload[target_member].as_i64(),
                Some(i64::from(value)),
                "case {case:?}: expected {target_member} {value}, got {payload}"
            );
        }
    }
}
