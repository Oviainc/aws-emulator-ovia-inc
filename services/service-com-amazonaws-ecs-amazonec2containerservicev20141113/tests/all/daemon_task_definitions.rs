// SPDX-License-Identifier: CC0-1.0

//! Daemon task definitions: registering a revision, and reading it back.
//!
//! The rules under test, each asserted over a table that spans its class rather than
//! over one request:
//!
//! * "Each time you call RegisterDaemonTaskDefinition, a new revision of the daemon task
//!   definition is created. You can't modify a revision after you register it" — a
//!   registration MINTS the next revision of its own family (1, 2, 3 …), stores what the
//!   request described, and answers with that revision's ARN;
//! * `DescribeDaemonTaskDefinition`: "You can specify a family and revision to find
//!   information about a specific daemon task definition, or you can simply specify the
//!   family to find the latest ACTIVE revision in that family" — every form of the
//!   reference (family, `family:revision`, `family:*`, full ARN) names the same stored
//!   record, and reports it as it was stored;
//! * `pidMode`/`ipcMode`: "The valid values are none and shared. The default is none. If
//!   none is specified or no value is provided, the daemon runs with its own PID
//!   namespace" — the mode is resolved at registration, so both ways of asking for the
//!   isolated namespace read back the same;
//! * a daemon task definition is its OWN resource: its ARN carries its own resource type
//!   and neither kind's reference resolves to the other kind's record.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION};

/// The values `DaemonPidMode` and `DaemonIpcMode` accept on the wire, as this crate
/// generated them from the model
/// (`smithy_query(query="shape", shape="DaemonPidMode,DaemonIpcMode")` lists `none` and
/// `shared` for both), so the catalog under test is the model's and not a copy of it.
use service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::{
    DAEMON_IPC_MODE_VALUES, DAEMON_PID_MODE_VALUES,
};

/// How many containers one definition may describe, and so the bound a registration is
/// tested at and one past.
const MAX_CONTAINERS: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_CONTAINER_DEFINITIONS;

/// The longest a family may be: "Up to 255 letters (uppercase and lowercase), numbers,
/// underscores, and hyphens are allowed" — the `family` member's own doc.
const MAX_FAMILY_LENGTH: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_NAME_LENGTH;

/// The image every container here starts from; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/aws-observability/aws-otel-collector:latest";

/// A minimal, valid daemon container definition.
fn daemon_container(name: &str, image: &str) -> Value {
    json!({ "name": name, "image": image, "memory": 128 })
}

/// The ARN a daemon revision of `family` carries, as
/// `arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:<revision>`.
fn daemon_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{family}:{revision}")
}

/// The ARN a PLAIN task-definition revision of `family` carries — the other resource,
/// used only to show that neither kind's reference reaches the other's record.
fn plain_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// The principal every request to this emulator authenticates as: the one IAM user its
/// `sts:GetCallerIdentity` publishes, which is what a "who registered this" member names.
fn caller_identity() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// Register one daemon revision and return the ARN the registration answered with.
async fn register(router: &MultiServiceRouter, request: Value, case: &str) -> String {
    let body = ok(router, "RegisterDaemonTaskDefinition", request, case).await;
    string(&body, "daemonTaskDefinitionArn", case)
}

/// Register one daemon revision of `family` with a single container named after `slot`,
/// so revisions are told apart by their CONTENT and not only by their number.
async fn register_revision(
    router: &MultiServiceRouter,
    family: &str,
    slot: i64,
    case: &str,
) -> String {
    register(
        router,
        json!({
            "family": family,
            "containerDefinitions": [daemon_container(&format!("agent{slot}"), IMAGE)],
        }),
        case,
    )
    .await
}

/// `DescribeDaemonTaskDefinition` for one reference, as the reader a registration is
/// proved through.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeDaemonTaskDefinition",
        json!({ "daemonTaskDefinition": reference }),
        case,
    )
    .await;
    match body.get("daemonTaskDefinition") {
        Some(value) if value.is_object() => value.clone(),
        other => {
            panic!("case {case:?}: expected a daemonTaskDefinition structure, got {other:?}")
        }
    }
}

/// One row of the registration table: how many containers it describes and the request
/// members it carries beyond `family` and `containerDefinitions`.
struct Row {
    case: &'static str,
    containers: usize,
    extra: Value,
}

#[tokio::test]
async fn a_registration_stores_the_revision_every_reference_form_reads_back() {
    let router = service();

    // The rows differ along what the rule generalises over: the NUMBER of containers, the
    // optional members the request carries (task-level sizing, the two roles, volumes,
    // tags) and the namespace modes it asks for.
    let rows = [
        Row {
            case: "one container and nothing optional",
            containers: 1,
            extra: json!({}),
        },
        Row {
            case: "task-level cpu and memory",
            containers: 1,
            extra: json!({ "cpu": "256", "memory": "512" }),
        },
        Row {
            case: "two containers and both roles",
            containers: 2,
            extra: json!({
                "taskRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/daemon-task"),
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/daemon-execution"),
            }),
        },
        Row {
            case: "the shared namespaces",
            containers: 1,
            extra: json!({ "pidMode": "shared", "ipcMode": "shared" }),
        },
        Row {
            case: "the isolated namespaces, named explicitly",
            containers: 1,
            extra: json!({ "pidMode": "none", "ipcMode": "none" }),
        },
        Row {
            case: "two volumes",
            containers: 1,
            extra: json!({
                "volumes": [
                    { "name": "logs", "host": { "sourcePath": "/var/log" } },
                    { "name": "scratch" },
                ],
            }),
        },
        Row {
            case: "three containers and tags",
            containers: 3,
            extra: json!({ "tags": [{ "key": "team", "value": "platform" }] }),
        },
        Row {
            case: "as many containers as one definition may describe",
            containers: MAX_CONTAINERS,
            extra: json!({}),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let family = format!("daemonfamily{index}");
        let containers: Vec<Value> = (0..row.containers)
            .map(|slot| daemon_container(&format!("agent{slot}"), IMAGE))
            .collect();
        let mut request = json!({ "family": family, "containerDefinitions": containers });
        for (member, value) in row
            .extra
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the row's extra members are an object"))
        {
            request[member.as_str()] = value.clone();
        }

        let arn = register(&router, request.clone(), case).await;
        assert_eq!(
            arn,
            daemon_arn(&family, 1),
            "case {case:?}: expected the first registration of {family:?} to mint \
             revision 1 under the daemon resource type, got {arn:?}"
        );

        // Every documented form of the reference names the SAME stored revision.
        let forms = [
            family.clone(),
            format!("{family}:1"),
            format!("{family}:*"),
            arn.clone(),
        ];
        let mut read: Vec<(String, Value)> = Vec::new();
        for form in &forms {
            let view = describe(&router, form, case).await;
            read.push((form.clone(), view));
        }
        let (first_form, view) = read[0].clone();
        for (form, other) in &read {
            assert_eq!(
                other, &view,
                "case {case:?}: {form:?} must name the same revision as {first_form:?}, \
                 got {other} against {view}"
            );
        }

        // What registration resolved, read back out of the store.
        assert_eq!(
            string(&view, "daemonTaskDefinitionArn", case),
            arn,
            "case {case:?}: expected the stored ARN back, got {view}"
        );
        assert_eq!(
            string(&view, "family", case),
            family,
            "case {case:?}: expected family {family:?} back, got {view}"
        );
        assert_eq!(
            count(&view, "revision", case),
            1,
            "case {case:?}: expected revision 1 back, got {view}"
        );
        assert_eq!(
            string(&view, "status", case),
            "ACTIVE",
            "case {case:?}: a registered revision is ACTIVE, got {view}"
        );
        assert_eq!(
            string(&view, "registeredBy", case),
            caller_identity(),
            "case {case:?}: expected the registering principal back, got {view}"
        );
        let registered_at = view
            .get("registeredAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a registeredAt, got {view}"));
        assert!(
            registered_at > 0.0,
            "case {case:?}: expected a registeredAt instant, got {registered_at}"
        );
        assert!(
            view.get("deleteRequestedAt").is_none(),
            "case {case:?}: no delete was requested, so the revision carries no \
             deleteRequestedAt, got {view}"
        );
        // The modelled `DaemonTaskDefinition` carries no tag set, so a registration that
        // sent tags reports none here.
        assert!(
            view.get("tags").is_none(),
            "case {case:?}: the daemon task definition shape has no tags member, got {view}"
        );

        // The containers the request described, in request order.
        let stored = list(&view, "containerDefinitions", case);
        assert_eq!(
            stored.len(),
            row.containers,
            "case {case:?}: expected {} container definitions back, got {view}",
            row.containers
        );
        for (slot, container) in stored.iter().enumerate() {
            assert_eq!(
                string(container, "name", case),
                format!("agent{slot}"),
                "case {case:?}: expected container {slot} to keep its name, got {container}"
            );
            assert_eq!(
                string(container, "image", case),
                IMAGE,
                "case {case:?}: expected container {slot} to keep its image, got {container}"
            );
            assert_eq!(
                count(container, "memory", case),
                128,
                "case {case:?}: expected container {slot} to keep its memory, got \
                 {container}"
            );
        }

        // A string member the request may or may not carry is reported exactly when it
        // was supplied — nothing is invented for one the caller left out.
        for member in ["cpu", "memory", "taskRoleArn", "executionRoleArn"] {
            let supplied = request.get(member).and_then(Value::as_str);
            let reported = view.get(member).and_then(Value::as_str);
            assert_eq!(
                reported, supplied,
                "case {case:?}: expected {member} {supplied:?}, got {reported:?} in {view}"
            );
        }

        // `volumes` is a list the projection always materialises, so a client may index
        // it unconditionally; its CONTENT is what the registration declared.
        let declared_volumes = request
            .get("volumes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert!(
            view.get("volumes").is_some(),
            "case {case:?}: volumes is always materialised, got {view}"
        );
        assert_eq!(
            list(&view, "volumes", case).len(),
            declared_volumes,
            "case {case:?}: expected {declared_volumes} volume(s) back, got {view}"
        );

        // "The default is none ... if no value is provided."
        for member in ["pidMode", "ipcMode"] {
            let expected = request.get(member).and_then(Value::as_str).unwrap_or("none");
            let reported = string(&view, member, case);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {member} {expected:?}, got {reported:?} in {view}"
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_namespace_mode_is_stored_and_the_other_member_keeps_its_default() {
    let router = service();

    // One row per member per value the model defines, plus the row that names nothing:
    // the member under test is read back as asked, and its neighbour takes the default.
    for (member, values) in [
        ("pidMode", DAEMON_PID_MODE_VALUES),
        ("ipcMode", DAEMON_IPC_MODE_VALUES),
    ] {
        let other = if member == "pidMode" {
            "ipcMode"
        } else {
            "pidMode"
        };
        for value in values.iter().copied().chain([""]) {
            let case = format!("{member} {value:?}");
            let family = format!(
                "daemonmode-{}-{}",
                member,
                if value.is_empty() { "default" } else { value }
            );
            let mut request = json!({
                "family": family,
                "containerDefinitions": [daemon_container("agent", IMAGE)],
            });
            if !value.is_empty() {
                request[member] = json!(value);
            }
            register(&router, request, &case).await;

            let view = describe(&router, &family, &case).await;
            let expected = if value.is_empty() { "none" } else { value };
            assert_eq!(
                string(&view, member, &case),
                expected,
                "case {case:?}: expected {member} {expected:?} back, got {view}"
            );
            assert_eq!(
                string(&view, other, &case),
                "none",
                "case {case:?}: expected the unasked {other} to take its documented \
                 default, got {view}"
            );
        }
    }
}

#[tokio::test]
async fn each_registration_of_a_family_mints_the_next_revision_of_that_family() {
    let router = service();

    // The families differ in SIZE and are registered INTERLEAVED, so a revision number
    // can come neither from a counter shared across families nor from the order of the
    // calls.
    let families = [
        ("daemoncount-one", 1_i64),
        ("daemoncount-two", 2),
        ("daemoncount-five", 5),
    ];
    let deepest = families
        .iter()
        .map(|(_, revisions)| *revisions)
        .max()
        .unwrap_or_default();
    for slot in 1..=deepest {
        for (family, revisions) in families {
            if slot > revisions {
                continue;
            }
            let case = format!("{family} registration {slot}");
            let arn = register_revision(&router, family, slot, &case).await;
            assert_eq!(
                arn,
                daemon_arn(family, slot),
                "case {case:?}: expected registration {slot} of {family:?} to mint \
                 revision {slot}, got {arn:?}"
            );
        }
    }

    for (family, revisions) in families {
        // Every revision is readable by its own number and keeps its own content: "You
        // can't modify a revision after you register it."
        for slot in 1..=revisions {
            let case = format!("{family} revision {slot} read back");
            let view = describe(&router, &format!("{family}:{slot}"), &case).await;
            assert_eq!(
                count(&view, "revision", &case),
                slot,
                "case {case:?}: expected revision {slot}, got {view}"
            );
            assert_eq!(
                string(&view, "daemonTaskDefinitionArn", &case),
                daemon_arn(family, slot),
                "case {case:?}: expected the revision's own ARN, got {view}"
            );
            let containers = list(&view, "containerDefinitions", &case);
            assert_eq!(
                containers.len(),
                1,
                "case {case:?}: expected one container, got {view}"
            );
            assert_eq!(
                string(&containers[0], "name", &case),
                format!("agent{slot}"),
                "case {case:?}: expected revision {slot} to keep the container it was \
                 registered with, got {view}"
            );
        }
        // The family alone resolves to "the latest ACTIVE revision in that family".
        let case = format!("{family} latest");
        let latest = describe(&router, family, &case).await;
        assert_eq!(
            count(&latest, "revision", &case),
            revisions,
            "case {case:?}: expected the family to resolve to revision {revisions}, got \
             {latest}"
        );
    }
}

#[tokio::test]
async fn a_registration_outside_a_documented_constraint_is_refused_and_stores_nothing() {
    let router = service();
    const FAMILY: &str = "daemonrefusals";
    register_revision(&router, FAMILY, 1, "the control registration").await;

    let past_family_length = "f".repeat(MAX_FAMILY_LENGTH + 1);
    let longest_family = "f".repeat(MAX_FAMILY_LENGTH);
    let past_container_quota: Vec<Value> = (0..=MAX_CONTAINERS)
        .map(|slot| daemon_container(&format!("agent{slot}"), IMAGE))
        .collect();
    let at_container_quota: Vec<Value> = (0..MAX_CONTAINERS)
        .map(|slot| daemon_container(&format!("agent{slot}"), IMAGE))
        .collect();
    let one_container = json!([daemon_container("agent", IMAGE)]);

    // The rows span the ways a registration can miss a constraint the members document:
    // the family's own, and the container list's.
    let rows: [(&str, &str, Value); 8] = [
        (
            "an empty family",
            "InvalidParameterException",
            json!({ "family": "", "containerDefinitions": one_container }),
        ),
        (
            "a family carrying a character outside the documented set",
            "InvalidParameterException",
            json!({ "family": "daemon family", "containerDefinitions": one_container }),
        ),
        (
            "a family one character past the documented length",
            "InvalidParameterException",
            json!({ "family": past_family_length, "containerDefinitions": one_container }),
        ),
        (
            "no container at all",
            "ClientException",
            json!({ "family": "daemonempty", "containerDefinitions": [] }),
        ),
        (
            "one container past the quota",
            "ClientException",
            json!({ "family": "daemontoomany", "containerDefinitions": past_container_quota }),
        ),
        (
            "two containers of one name",
            "ClientException",
            json!({
                "family": "daemonduplicate",
                "containerDefinitions": [
                    daemon_container("agent", IMAGE),
                    daemon_container("agent", IMAGE),
                ],
            }),
        ),
        (
            "a container with no name",
            "ClientException",
            json!({
                "family": "daemonnoname",
                "containerDefinitions": [{ "image": IMAGE, "memory": 128 }],
            }),
        ),
        (
            "a container with a blank image",
            "ClientException",
            json!({
                "family": "daemonnoimage",
                "containerDefinitions": [{ "name": "agent", "image": "   ", "memory": 128 }],
            }),
        ),
    ];
    for (case, expected_code, request) in rows {
        let (code, message) =
            refusal(&router, "RegisterDaemonTaskDefinition", request, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected a {expected_code}, got {code} with {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {message:?}"
        );
    }

    // The published bounds themselves are accepted: a family of the documented length,
    // and exactly the quota's worth of containers.
    let at_bound = register(
        &router,
        json!({
            "family": longest_family,
            "containerDefinitions": one_container,
        }),
        "a family of the documented length",
    )
    .await;
    assert_eq!(
        at_bound,
        daemon_arn(&longest_family, 1),
        "expected a family of the documented length to be registered, got {at_bound:?}"
    );
    let at_quota = register(
        &router,
        json!({
            "family": "daemonatquota",
            "containerDefinitions": at_container_quota,
        }),
        "the quota's worth of containers",
    )
    .await;
    assert_eq!(
        at_quota,
        daemon_arn("daemonatquota", 1),
        "expected the quota's worth of containers to be registered, got {at_quota:?}"
    );

    // A refused registration stored nothing: none of the families it named is readable.
    for family in [
        "daemonempty",
        "daemontoomany",
        "daemonduplicate",
        "daemonnoname",
        "daemonnoimage",
    ] {
        let (code, message) = refusal(
            &router,
            "DescribeDaemonTaskDefinition",
            json!({ "daemonTaskDefinition": family }),
            family,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {family:?}: a refused registration stored nothing, so the family \
             resolves to no revision; got {code} with {message:?}"
        );
    }

    // And it consumed no revision number of the family that WAS registered.
    let next = register_revision(&router, FAMILY, 2, "after the refusals").await;
    assert_eq!(
        next,
        daemon_arn(FAMILY, 2),
        "expected the next registration of {FAMILY:?} to mint revision 2, got {next:?}"
    );
}

#[tokio::test]
async fn a_reference_that_names_no_stored_daemon_revision_is_refused() {
    let router = service();
    const FAMILY: &str = "daemonreader";
    register_revision(&router, FAMILY, 1, "the daemon revision").await;
    // A PLAIN task definition registered under the SAME family name: a different
    // resource, so neither kind's reference may reach the other's record.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [daemon_container("app", IMAGE)],
        }),
        "the plain revision",
    )
    .await;

    // The rows span the ways a reference can fail to name one stored daemon revision.
    let rows = [
        (
            "a family nothing was registered under",
            "neverregistered".to_string(),
        ),
        (
            "a revision the family never minted",
            format!("{FAMILY}:9"),
        ),
        ("revision zero", format!("{FAMILY}:0")),
        ("not a reference at all", "not a reference".to_string()),
        (
            "the daemon ARN of another family",
            daemon_arn("neverregistered", 1),
        ),
        (
            "the ARN of the plain revision of the same family",
            plain_arn(FAMILY, 1),
        ),
    ];
    for (case, reference) in rows {
        let (code, message) = refusal(
            &router,
            "DescribeDaemonTaskDefinition",
            json!({ "daemonTaskDefinition": reference.clone() }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: expected a ClientException for {reference:?}, got {code} \
             with {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {message:?}"
        );
    }

    // The other way round: a daemon revision's ARN names no plain revision either.
    let case = "a daemon ARN read as a plain revision";
    let (code, message) = refusal(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": daemon_arn(FAMILY, 1) }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected a ClientException, got {code} with {message:?}"
    );

    // The control: both revisions are still readable, each through its own operation.
    let daemon = describe(&router, FAMILY, "the control reads").await;
    assert_eq!(
        string(&daemon, "daemonTaskDefinitionArn", "the control reads"),
        daemon_arn(FAMILY, 1),
        "expected the daemon revision to be readable, got {daemon}"
    );
    let plain = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": FAMILY }),
        "the control reads",
    )
    .await;
    assert_eq!(
        string(&plain["taskDefinition"], "taskDefinitionArn", "the control reads"),
        plain_arn(FAMILY, 1),
        "expected the plain revision to be readable, got {plain}"
    );
}
