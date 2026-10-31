// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Mac modification task slice:
//! `CreateMacSystemIntegrityProtectionModificationTask`,
//! `CreateDelegateMacVolumeOwnershipTask` and `DescribeMacModificationTasks`, driven over
//! the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a create MINTS A TASK FROM THE REQUEST and `DescribeMacModificationTasks` REPORTS THE
//!    STORE: every member the request sets round-trips through the describe, and the members
//!    the service owns (the task id, the start time, the state and the task type) are
//!    derived rather than taken;
//! 2. the SIP configuration a task reports is the overall `MacSystemIntegrityProtectionStatus`
//!    for every setting, with the exceptions `MacSystemIntegrityProtectionConfiguration`
//!    names overriding it;
//! 3. the task is handed back in the non-terminal `pending` state and the CLOCK owes it
//!    `in-progress` and then `successful`
//!    (`smithy_query(query="shape",shape="MacModificationTaskState")`), with the describe
//!    writing nothing at all;
//! 4. the instance a task names is RESOLVED before a task is stored — it has to be an
//!    Amazon EC2 Mac instance of the account, and a volume ownership delegation needs an
//!    APPLE SILICON one;
//! 5. one `ClientToken` names one task, and a different request under a token already in use
//!    is refused;
//! 6. the describe narrows by `MacModificationTaskIds` and by every filter it answers, and
//!    pages in task id order by `MaxResults`/`NextToken`, refusing a page size outside the
//!    range the model publishes;
//! 7. `DryRun` reports whether the call WOULD have succeeded and stores nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which Mac
//! family, which of the two creates, which overall SIP status, how many exceptions, how many
//! tags, which filter, how many tasks, which page size — so no row is the specification. The
//! instance types come from the modelled `InstanceType` catalog and the SIP statuses from the
//! modelled `MacSystemIntegrityProtectionSettingStatus` catalog, so nothing here is a list
//! written by hand.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::ops::types::{
    MAC_MODIFICATION_TASK_STATE_VALUES, MAC_MODIFICATION_TASK_TYPE_VALUES,
    MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES,
};
use ec2::state::{
    architecture_for, instance_family_of, is_mac_instance_family, offered_instance_types,
    ARCHITECTURE_ARM64_MAC, MAC_MODIFICATION_TASK_STATE_IN_PROGRESS,
    MAC_MODIFICATION_TASK_STATE_PENDING, MAC_MODIFICATION_TASK_STATE_SUCCESSFUL,
    MAC_MODIFICATION_TASK_TYPE_SIP_MODIFICATION,
    MAC_MODIFICATION_TASK_TYPE_VOLUME_OWNERSHIP_DELEGATION, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed};

/// The Region every call here is signed for. A Mac modification task carries no Region
/// member, so the Region only selects the envelope the service answers in.
const REGION: &str = "us-west-2";

/// An AMI id of the long form, for the launches these rows need: the launch path does not
/// resolve the image, and nothing about a Mac modification task reads it.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The smallest page `DescribeMacModificationTasks` can be ASKED for: the bottom of the
/// modelled `MaxResults` range, `range 1..500`
/// (`smithy_query(query="operation",operation="DescribeMacModificationTasks")`).
const PUBLISHED_MIN_PAGE: i64 = 1;

/// The largest page it can be asked for: the top of that same published range.
const PUBLISHED_MAX_PAGE: i64 = 500;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (String, Params, &'static str);

/// Each individual SIP setting as a request names it and as a task reports it: the member of
/// `MacSystemIntegrityProtectionConfigurationRequest` and the `xmlName` of the matching
/// member of `MacSystemIntegrityProtectionConfiguration`
/// (`smithy_query(query="shape",shape="MacSystemIntegrityProtectionConfiguration,MacSystemIntegrityProtectionConfigurationRequest")`).
const SIP_SETTINGS: [(&str, &str); 7] = [
    ("AppleInternal", "appleInternal"),
    ("BaseSystem", "baseSystem"),
    ("DebuggingRestrictions", "debuggingRestrictions"),
    ("DTraceRestrictions", "dTraceRestrictions"),
    ("FilesystemProtections", "filesystemProtections"),
    ("KextSigning", "kextSigning"),
    ("NvramProtections", "nvramProtections"),
];

/// The `xmlName` of `MacSystemIntegrityProtectionConfiguration$Status` — "indicates SIP was
/// enabled or disabled by the task".
const SIP_STATUS_MEMBER: &str = "status";

/// Credentials of the shape `MacCredentials` documents, for the rows that have to carry the
/// member: the operation stores none of it and reports none of it, so its only job is to be
/// present.
const MAC_CREDENTIALS: &str = "{\"internalDiskPassword\":\"\",\"rootVolumeUsername\":\"ec2-user\"}";

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_one_move() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// Wait until both moves a fresh task is owed — `pending` -> `in-progress` -> `successful` —
/// have certainly fallen due.
async fn past_both_moves() {
    tokio::time::sleep(TRANSITION_DELAY * 2 + Duration::from_millis(200)).await;
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

async fn create_sip(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(
        router,
        "CreateMacSystemIntegrityProtectionModificationTask",
        params,
    )
    .await
}

async fn create_delegation(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    api(router, "CreateDelegateMacVolumeOwnershipTask", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeMacModificationTasks", params).await
}

/// The Mac instance types the model declares, in catalog order: every type this service
/// offers (`offered_instance_types`, the modelled `InstanceType` enum) whose instance family
/// runs macOS. Read off the model rather than written here, so a family the model adds or
/// drops moves with it.
fn mac_instance_types() -> Vec<&'static str> {
    offered_instance_types()
        .iter()
        .copied()
        .filter(|instance_type| is_mac_instance_family(instance_family_of(instance_type)))
        .collect()
}

/// The Apple silicon Mac instance types: the Mac types whose architecture is `arm64_mac`.
fn apple_silicon_mac_types() -> Vec<&'static str> {
    mac_instance_types()
        .into_iter()
        .filter(|instance_type| architecture_for(instance_type) == ARCHITECTURE_ARM64_MAC)
        .collect()
}

/// The Intel Mac instance types: the Mac types whose architecture is anything else.
fn intel_mac_types() -> Vec<&'static str> {
    mac_instance_types()
        .into_iter()
        .filter(|instance_type| architecture_for(instance_type) != ARCHITECTURE_ARM64_MAC)
        .collect()
}

/// Launch one instance of `instance_type` and answer its id.
async fn launch(router: &MultiServiceRouter, instance_type: &str) -> String {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", IMAGE_ID),
            p("InstanceType", instance_type),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances for {instance_type} must succeed, got {status}: {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// One reported `macModificationTask`, read out of the XML.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Task {
    id: Option<String>,
    instance_id: Option<String>,
    task_state: Option<String>,
    task_type: Option<String>,
    start_time: Option<String>,
    sip_configuration: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl Task {
    /// The members no clock-owed move can change — what a round-trip assertion compares.
    fn settled(&self) -> Task {
        Task {
            task_state: None,
            ..self.clone()
        }
    }
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|inner| inner.is_element() && inner.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|inner| inner.text().unwrap_or_default().to_string())
}

fn task_of(node: roxmltree::Node<'_, '_>) -> Task {
    Task {
        id: child_text(node, "macModificationTaskId"),
        instance_id: child_text(node, "instanceId"),
        task_state: child_text(node, "taskState"),
        task_type: child_text(node, "taskType"),
        start_time: child_text(node, "startTime"),
        sip_configuration: child(node, "macSystemIntegrityProtectionConfig")
            .map(|configuration| {
                configuration
                    .children()
                    .filter(roxmltree::Node::is_element)
                    .map(|setting| {
                        (
                            setting.tag_name().name().to_string(),
                            setting.text().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        tags: child(node, "tagSet")
            .map(|set| {
                set.children()
                    .filter(roxmltree::Node::is_element)
                    .filter_map(|item| Some((child_text(item, "key")?, child_text(item, "value")?)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The task a create answered with.
fn created_task(body: &str) -> Task {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    let node = child(document.root_element(), "macModificationTask")
        .unwrap_or_else(|| panic!("a create must report the task it started: {body}"));
    task_of(node)
}

/// Every task a describe reported, in document order.
fn described_tasks(body: &str) -> Vec<Task> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    child(document.root_element(), "macModificationTaskSet")
        .map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .map(task_of)
                .collect()
        })
        .unwrap_or_default()
}

/// Every task the store holds, by id, in the order the describe reports them.
async fn all_task_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "DescribeMacModificationTasks must succeed, got {status}: {body}"
    );
    described_tasks(&body)
        .into_iter()
        .filter_map(|task| task.id)
        .collect()
}

/// A `MacModificationTaskId` is `macmdf-` followed by 8 or 17 lowercase hex digits, the
/// short and the long EC2 id form.
fn is_task_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("macmdf-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An RFC 3339 UTC timestamp, as the service renders `StartTime`: `YYYY-MM-DDThh:mm:ssZ`.
fn is_iso8601_utc(candidate: &str) -> bool {
    candidate.ends_with('Z')
        && candidate.len() >= "0000-00-00T00:00:00Z".len()
        && candidate.as_bytes()[4] == b'-'
        && candidate.as_bytes()[7] == b'-'
        && candidate.as_bytes()[10] == b'T'
        && candidate.as_bytes()[13] == b':'
}

/// Which create a row drives, and with what.
#[derive(Debug, Clone)]
enum Kind {
    /// A SIP modification task: the overall status, and the exceptions the request names as
    /// `(request member, value)`.
    Sip {
        status: &'static str,
        exceptions: Vec<(&'static str, &'static str)>,
    },
    /// A volume ownership delegation task.
    Delegation,
}

/// One create form: which instance it runs on, which create, and the tags it asks for.
#[derive(Debug, Clone)]
struct Case {
    label: String,
    instance_type: &'static str,
    kind: Kind,
    tags: Vec<(&'static str, &'static str)>,
}

/// The SIP configuration a task must report for `kind`, by reported member name: every
/// setting takes the overall status unless the request named an exception for it, and
/// `status` is the overall status itself.
fn expected_sip_configuration(kind: &Kind) -> BTreeMap<String, String> {
    match kind {
        Kind::Delegation => BTreeMap::new(),
        Kind::Sip { status, exceptions } => {
            let mut expected: BTreeMap<String, String> = SIP_SETTINGS
                .iter()
                .map(|(requested, reported)| {
                    let value = exceptions
                        .iter()
                        .find(|(member, _)| member == requested)
                        .map_or(*status, |(_, value)| *value);
                    ((*reported).to_string(), value.to_string())
                })
                .collect();
            expected.insert(SIP_STATUS_MEMBER.to_string(), (*status).to_string());
            expected
        }
    }
}

/// The parameters `case` adds beside its `InstanceId`.
fn case_params(case: &Case) -> Params {
    let mut params = Vec::new();
    match &case.kind {
        Kind::Sip { status, exceptions } => {
            params.push(p("MacSystemIntegrityProtectionStatus", *status));
            for (member, value) in exceptions {
                params.push(p(
                    &format!("MacSystemIntegrityProtectionConfiguration.{member}"),
                    *value,
                ));
            }
        }
        Kind::Delegation => params.push(p("MacCredentials", MAC_CREDENTIALS)),
    }
    if !case.tags.is_empty() {
        params.push(p(
            "TagSpecification.1.ResourceType",
            "mac-modification-task",
        ));
        for (index, (key, value)) in case.tags.iter().enumerate() {
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                *key,
            ));
            params.push(p(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                *value,
            ));
        }
    }
    params
}

/// Run `case`'s create against `instance_id` and answer the response.
async fn run_case(
    router: &MultiServiceRouter,
    case: &Case,
    instance_id: &str,
    extra: Params,
) -> (u16, String) {
    let mut params = vec![p("InstanceId", instance_id)];
    params.extend(case_params(case));
    params.extend(extra);
    match case.kind {
        Kind::Sip { .. } => create_sip(router, &params).await,
        Kind::Delegation => create_delegation(router, &params).await,
    }
}

/// Rules 1 and 2: each create mints the task its request describes, with the SIP
/// configuration the overall status and the exceptions imply, and the describe reports the
/// same task back.
#[tokio::test]
async fn a_created_task_round_trips_through_the_describe() {
    // "disabled" and "enabled", from the modelled
    // `MacSystemIntegrityProtectionSettingStatus` catalog rather than written here.
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let apple_silicon = apple_silicon_mac_types();
    let intel = intel_mac_types();
    assert!(
        apple_silicon.len() >= 2 && !intel.is_empty(),
        "the model must declare at least two Apple silicon Mac types and one Intel one, got \
         {apple_silicon:?} and {intel:?}"
    );

    let cases: Vec<Case> = vec![
        Case {
            label: format!("all SIP settings {} on {}", statuses[1], apple_silicon[0]),
            instance_type: apple_silicon[0],
            kind: Kind::Sip {
                status: statuses[1],
                exceptions: Vec::new(),
            },
            tags: Vec::new(),
        },
        Case {
            label: format!("all SIP settings {} on the Intel {}", statuses[0], intel[0]),
            instance_type: intel[0],
            kind: Kind::Sip {
                status: statuses[0],
                exceptions: Vec::new(),
            },
            tags: vec![("Name", "sip-off"), ("Tier", "batch")],
        },
        Case {
            label: format!("SIP {} except NvramProtections", statuses[1]),
            instance_type: apple_silicon[1],
            kind: Kind::Sip {
                status: statuses[1],
                exceptions: vec![("NvramProtections", statuses[0])],
            },
            tags: vec![("Name", "one-exception")],
        },
        Case {
            label: format!("SIP {} with three exceptions", statuses[0]),
            instance_type: apple_silicon[0],
            kind: Kind::Sip {
                status: statuses[0],
                exceptions: vec![
                    ("DTraceRestrictions", statuses[1]),
                    ("KextSigning", statuses[1]),
                    ("AppleInternal", statuses[1]),
                ],
            },
            tags: Vec::new(),
        },
        Case {
            label: format!("a volume ownership delegation on {}", apple_silicon[0]),
            instance_type: apple_silicon[0],
            kind: Kind::Delegation,
            tags: vec![("Name", "delegate")],
        },
        Case {
            label: format!(
                "a volume ownership delegation on {} with no tags",
                apple_silicon[1]
            ),
            instance_type: apple_silicon[1],
            kind: Kind::Delegation,
            tags: Vec::new(),
        },
    ];

    for case in &cases {
        let router = emulator();
        let instance_id = launch(&router, case.instance_type).await;
        let (status, body) = run_case(&router, case, &instance_id, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {:?}: the create must succeed, got {status}: {body}",
            case.label
        );
        let created = created_task(&body);

        let task_id = created
            .id
            .clone()
            .unwrap_or_else(|| panic!("case {:?}: the task must carry an id: {body}", case.label));
        assert!(
            is_task_id(&task_id),
            "case {:?}: expected a macmdf- task id, got {task_id:?}",
            case.label
        );
        assert_eq!(
            created.instance_id.as_deref(),
            Some(instance_id.as_str()),
            "case {:?}: the task must name the instance it was created for, got {:?}",
            case.label,
            created.instance_id
        );
        assert_eq!(
            created.task_state.as_deref(),
            Some(MAC_MODIFICATION_TASK_STATE_PENDING),
            "case {:?}: a fresh task is pending, got {:?}",
            case.label,
            created.task_state
        );
        let expected_type = match case.kind {
            Kind::Sip { .. } => MAC_MODIFICATION_TASK_TYPE_SIP_MODIFICATION,
            Kind::Delegation => MAC_MODIFICATION_TASK_TYPE_VOLUME_OWNERSHIP_DELEGATION,
        };
        assert!(
            MAC_MODIFICATION_TASK_TYPE_VALUES.contains(&expected_type),
            "case {:?}: {expected_type} must be a modelled task type, the catalog is {:?}",
            case.label,
            MAC_MODIFICATION_TASK_TYPE_VALUES
        );
        assert_eq!(
            created.task_type.as_deref(),
            Some(expected_type),
            "case {:?}: the task must report the type the operation creates, got {:?}",
            case.label,
            created.task_type
        );
        let start_time = created.start_time.clone().unwrap_or_else(|| {
            panic!(
                "case {:?}: the task must report when it was created: {body}",
                case.label
            )
        });
        assert!(
            is_iso8601_utc(&start_time),
            "case {:?}: expected a UTC timestamp for startTime, got {start_time:?}",
            case.label
        );
        assert_eq!(
            created.sip_configuration,
            expected_sip_configuration(&case.kind),
            "case {:?}: the SIP configuration the task reports is wrong",
            case.label
        );
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {:?}: the task must carry the tags the request asked for",
            case.label
        );

        // The describe reports the SAME task, by id and in the unnarrowed answer.
        let (status, body) = describe(&router, &[p("MacModificationTaskId.1", task_id.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the describe must succeed, got {status}: {body}",
            case.label
        );
        let reported = described_tasks(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected exactly the one task, got {reported:?}",
            case.label
        );
        assert_eq!(
            reported[0].settled(),
            created.settled(),
            "case {:?}: the describe must report the task the create minted",
            case.label
        );
        assert_eq!(
            all_task_ids(&router).await,
            vec![task_id.clone()],
            "case {:?}: the store must hold exactly the one task",
            case.label
        );
    }
}

/// Rule 3: the clock walks a task through its states, and the describe writes nothing.
#[tokio::test]
async fn the_clock_moves_a_task_to_its_terminal_state() {
    let router = emulator();
    let instance_type = apple_silicon_mac_types()[0];
    let instance_id = launch(&router, instance_type).await;
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;

    let (status, body) = create_sip(
        &router,
        &[
            p("InstanceId", instance_id.as_str()),
            p("MacSystemIntegrityProtectionStatus", statuses[1]),
        ],
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
    let created = created_task(&body);
    let task_id = created.id.clone().expect("the task must carry an id");

    // Every state the walk reports is a member of the modelled catalog.
    for state in [
        MAC_MODIFICATION_TASK_STATE_PENDING,
        MAC_MODIFICATION_TASK_STATE_IN_PROGRESS,
        MAC_MODIFICATION_TASK_STATE_SUCCESSFUL,
    ] {
        assert!(
            MAC_MODIFICATION_TASK_STATE_VALUES.contains(&state),
            "case \"the state catalog\": {state} must be a modelled task state, the catalog \
             is {MAC_MODIFICATION_TASK_STATE_VALUES:?}"
        );
    }

    assert_eq!(
        created.task_state.as_deref(),
        Some(MAC_MODIFICATION_TASK_STATE_PENDING),
        "case \"straight after the create\": the task must still be pending, got {:?}",
        created.task_state
    );

    // (what the clock has reached, the state the task must report)
    for (label, expected) in [
        (
            "one span after the create",
            MAC_MODIFICATION_TASK_STATE_IN_PROGRESS,
        ),
        (
            "two spans after the create",
            MAC_MODIFICATION_TASK_STATE_SUCCESSFUL,
        ),
    ] {
        past_one_move().await;
        let (status, body) =
            describe(&router, &[p("MacModificationTaskId.1", task_id.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the describe must succeed, got {status}: {body}"
        );
        let reported = described_tasks(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {label:?}: expected the one task, got {reported:?}"
        );
        assert_eq!(
            reported[0].task_state.as_deref(),
            Some(expected),
            "case {label:?}: expected the task in {expected}, got {:?}",
            reported[0].task_state
        );
    }

    // A read changes nothing a later caller sees: two describes of a settled task agree, and
    // the task the create reported still round-trips.
    let (_, first) = describe(&router, &[]).await;
    let (_, second) = describe(&router, &[]).await;
    assert_eq!(
        described_tasks(&first),
        described_tasks(&second),
        "case \"two reads of a settled task\": a describe must write nothing"
    );
    assert_eq!(
        described_tasks(&first)[0].settled(),
        created.settled(),
        "case \"two reads of a settled task\": only the state may have moved"
    );
}

/// Rule 4: the instance a task names has to be an EC2 Mac instance of the account, an Apple
/// silicon one for a volume ownership delegation, and one that is not on its way to
/// `terminated`.
#[tokio::test]
async fn the_instance_a_task_names_is_resolved_first() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let apple_silicon = apple_silicon_mac_types()[0];
    let intel = intel_mac_types()[0];

    let mac_instance = launch(&router, apple_silicon).await;
    let intel_instance = launch(&router, intel).await;
    // A type of no Mac family at all, taken from the modelled catalog.
    let other_type = offered_instance_types()
        .iter()
        .copied()
        .find(|instance_type| !is_mac_instance_family(instance_family_of(instance_type)))
        .expect("the model must declare an instance type that is not a Mac");
    let other_instance = launch(&router, other_type).await;

    let sip = |instance: &str| {
        vec![
            p("InstanceId", instance),
            p("MacSystemIntegrityProtectionStatus", statuses[1]),
        ]
    };

    let rows: Vec<RefusalRow> = vec![
        (
            "an instance id outside the form EC2 issues".to_string(),
            sip("i-not-an-id"),
            "InvalidInstanceID.Malformed",
        ),
        (
            "an instance id of the issued form that names nothing".to_string(),
            sip("i-0fedcba987654321f"),
            "InvalidInstanceID.NotFound",
        ),
        (
            format!("an instance of the {other_type} family, which runs no macOS"),
            sip(other_instance.as_str()),
            "InvalidParameterValue",
        ),
    ];
    for (label, params, code) in rows {
        let (status, body) = create_sip(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }

    // The delegation needs an APPLE SILICON Mac: the Intel Mac is a Mac instance the SIP
    // create serves and the delegation refuses, which is the difference between the two.
    let (status, body) = create_sip(&router, &sip(intel_instance.as_str())).await;
    assert_eq!(
        status, 200,
        "case \"a SIP task on an Intel Mac\": the create must succeed, got {status}: {body}"
    );
    for (label, instance, expected_status, code) in [
        (
            "a delegation on an Intel Mac",
            intel_instance.as_str(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a delegation on an instance of no Mac family",
            other_instance.as_str(),
            400,
            "InvalidParameterValue",
        ),
        (
            "a delegation on an Apple silicon Mac",
            mac_instance.as_str(),
            200,
            "",
        ),
    ] {
        let (status, body) = create_delegation(
            &router,
            &[p("InstanceId", instance), p("MacCredentials", MAC_CREDENTIALS)],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status}: {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                code,
                "case {label:?}: expected {code}, got {body}"
            );
        }
    }

    // An instance on its way to `terminated` has no macOS left to configure.
    let (status, body) = api(
        &router,
        "TerminateInstances",
        &[p("InstanceId.1", mac_instance.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "TerminateInstances must succeed, got {status}: {body}"
    );
    let (status, body) = create_sip(&router, &sip(mac_instance.as_str())).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectInstanceState"),
        "case \"an instance on its way to terminated\": expected 400 IncorrectInstanceState, \
         got {status}: {body}"
    );
}

/// Rule 5: one `ClientToken` names one task, and a token already naming one task is not
/// reused for a different request.
#[tokio::test]
async fn one_client_token_names_one_task() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let apple_silicon = apple_silicon_mac_types();
    let first_instance = launch(&router, apple_silicon[0]).await;
    let second_instance = launch(&router, apple_silicon[1]).await;

    let params = vec![
        p("InstanceId", first_instance.as_str()),
        p("MacSystemIntegrityProtectionStatus", statuses[1]),
        p("ClientToken", "sip-once"),
    ];
    let (status, body) = create_sip(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first create must succeed, got {status}: {body}"
    );
    let first = created_task(&body).id.expect("the task must carry an id");

    for attempt in 1..=2 {
        let (status, body) = create_sip(&router, &params).await;
        assert_eq!(
            status, 200,
            "case \"replay {attempt}\": a replay must succeed, got {status}: {body}"
        );
        assert_eq!(
            created_task(&body).id.as_deref(),
            Some(first.as_str()),
            "case \"replay {attempt}\": a replay must report the task the token names"
        );
    }
    assert_eq!(
        all_task_ids(&router).await,
        vec![first.clone()],
        "a replay must start no second task"
    );

    // The SAME token with a different request is not a replay, whatever differs — the
    // instance, the overall SIP status, the tags or the operation.
    for (label, params) in [
        (
            "the same token on another instance",
            vec![
                p("InstanceId", second_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p("ClientToken", "sip-once"),
            ],
        ),
        (
            "the same token on another instance and status",
            vec![
                p("InstanceId", second_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[0]),
                p("ClientToken", "sip-once"),
            ],
        ),
        (
            "the same token with the other overall SIP status",
            vec![
                p("InstanceId", first_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[0]),
                p("ClientToken", "sip-once"),
            ],
        ),
        (
            "the same token with one SIP setting excepted",
            vec![
                p("InstanceId", first_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p(
                    "MacSystemIntegrityProtectionConfiguration.NvramProtections",
                    statuses[0],
                ),
                p("ClientToken", "sip-once"),
            ],
        ),
        (
            "the same token with a tag the first request did not ask for",
            vec![
                p("InstanceId", first_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p("TagSpecification.1.ResourceType", "mac-modification-task"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "second-thoughts"),
                p("ClientToken", "sip-once"),
            ],
        ),
    ] {
        let (status, body) = create_sip(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "IdempotentParameterMismatch"),
            "case {label:?}: expected 400 IdempotentParameterMismatch, got {status}: {body}"
        );
    }
    let (status, body) = create_delegation(
        &router,
        &[
            p("InstanceId", first_instance.as_str()),
            p("MacCredentials", MAC_CREDENTIALS),
            p("ClientToken", "sip-once"),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IdempotentParameterMismatch"),
        "case \"the same token for a task of another type\": expected 400 \
         IdempotentParameterMismatch, got {status}: {body}"
    );

    // A different token is a different request, so it starts its own task.
    let (status, body) = create_delegation(
        &router,
        &[
            p("InstanceId", first_instance.as_str()),
            p("MacCredentials", MAC_CREDENTIALS),
            p("ClientToken", "delegate-once"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the second create must succeed, got {status}: {body}"
    );
    let second = created_task(&body).id.expect("the task must carry an id");
    assert_ne!(
        second, first,
        "a second token must name a task of its own, got {second} twice"
    );
}

/// Rule 6: the describe narrows by id and by every filter it answers, and refuses an id it
/// cannot resolve and a filter it does not document.
#[tokio::test]
async fn the_describe_narrows_by_id_and_by_filter() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let apple_silicon = apple_silicon_mac_types();
    let first_instance = launch(&router, apple_silicon[0]).await;
    let second_instance = launch(&router, apple_silicon[1]).await;

    // Two SIP tasks on the first instance (one enabled, one disabled) and a delegation on
    // the second, tagged so the tag filters have something to select on.
    let mut sip_enabled = Vec::new();
    for (index, status_value) in [statuses[1], statuses[0]].into_iter().enumerate() {
        let (status, body) = create_sip(
            &router,
            &[
                p("InstanceId", first_instance.as_str()),
                p("MacSystemIntegrityProtectionStatus", status_value),
                p("ClientToken", format!("sip-{index}")),
                p("TagSpecification.1.ResourceType", "mac-modification-task"),
                p("TagSpecification.1.Tag.1.Key", "Tier"),
                p(
                    "TagSpecification.1.Tag.1.Value",
                    if index == 0 { "web" } else { "batch" },
                ),
            ],
        )
        .await;
        assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
        sip_enabled.push(created_task(&body).id.expect("the task must carry an id"));
    }
    let (status, body) = create_delegation(
        &router,
        &[
            p("InstanceId", second_instance.as_str()),
            p("MacCredentials", MAC_CREDENTIALS),
            p("TagSpecification.1.ResourceType", "mac-modification-task"),
            p("TagSpecification.1.Tag.1.Key", "Owner"),
            p("TagSpecification.1.Tag.1.Value", "platform"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
    let delegation = created_task(&body).id.expect("the task must carry an id");

    let sip_on = sip_enabled[0].clone();
    let sip_off = sip_enabled[1].clone();
    let mut every_id = vec![sip_on.clone(), sip_off.clone(), delegation.clone()];
    every_id.sort();

    // Narrowing by id reports exactly the ids named, in task id order.
    let (status, body) = describe(
        &router,
        &indexed("MacModificationTaskId", &[&sip_off, &sip_on]),
    )
    .await;
    assert_eq!(status, 200, "the describe must succeed, got {status}: {body}");
    let mut reported: Vec<String> = described_tasks(&body)
        .into_iter()
        .filter_map(|task| task.id)
        .collect();
    let mut expected = vec![sip_on.clone(), sip_off.clone()];
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"two ids\": the describe must report the tasks named, in id order"
    );

    // The start time a filter row compares against is read off a reported task, never
    // written here.
    let (_, body) = describe(&router, &[p("MacModificationTaskId.1", delegation.as_str())]).await;
    let delegation_start = described_tasks(&body)[0]
        .start_time
        .clone()
        .expect("the task must report when it was created");

    // (what the filter asks for, the filter entries, the ids it must report)
    let mut rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "instance-id names the first instance".to_string(),
            filters(&[("instance-id", &[first_instance.as_str()])]),
            vec![sip_off.clone(), sip_on.clone()],
        ),
        (
            "instance-id names either instance (values joined with OR)".to_string(),
            filters(&[(
                "instance-id",
                &[first_instance.as_str(), second_instance.as_str()],
            )]),
            every_id.clone(),
        ),
        (
            "task-type names the delegation".to_string(),
            filters(&[(
                "task-type",
                &[MAC_MODIFICATION_TASK_TYPE_VOLUME_OWNERSHIP_DELEGATION],
            )]),
            vec![delegation.clone()],
        ),
        (
            "task-type names the SIP modifications".to_string(),
            filters(&[("task-type", &[MAC_MODIFICATION_TASK_TYPE_SIP_MODIFICATION])]),
            vec![sip_off.clone(), sip_on.clone()],
        ),
        (
            format!("the SIP status the task requested is {}", statuses[1]),
            filters(&[(
                "mac-system-integrity-protection-configuration.sip-status",
                &[statuses[1]],
            )]),
            vec![sip_on.clone()],
        ),
        (
            format!("the SIP status the task requested is {}", statuses[0]),
            filters(&[(
                "mac-system-integrity-protection-configuration.sip-status",
                &[statuses[0]],
            )]),
            vec![sip_off.clone()],
        ),
        (
            "task-state is pending, which every fresh task is".to_string(),
            filters(&[("task-state", &[MAC_MODIFICATION_TASK_STATE_PENDING])]),
            every_id.clone(),
        ),
        (
            "task-state is the terminal one, which none has reached".to_string(),
            filters(&[("task-state", &[MAC_MODIFICATION_TASK_STATE_SUCCESSFUL])]),
            Vec::new(),
        ),
        (
            "tag-key names the key only the delegation carries".to_string(),
            filters(&[("tag-key", &["Owner"])]),
            vec![delegation.clone()],
        ),
        (
            "tag:Tier selects one value of the key two tasks carry".to_string(),
            filters(&[("tag:Tier", &["batch"])]),
            vec![sip_off.clone()],
        ),
        (
            "tag:Tier with an EC2 wildcard selects both".to_string(),
            filters(&[("tag:Tier", &["*b*"])]),
            vec![sip_off.clone(), sip_on.clone()],
        ),
        (
            "start-time names the instant the delegation was created".to_string(),
            filters(&[("start-time", &[delegation_start.as_str()])]),
            vec![delegation.clone()],
        ),
        (
            "two filters are joined with AND".to_string(),
            filters(&[
                ("instance-id", &[first_instance.as_str()]),
                ("tag:Tier", &["web"]),
            ]),
            vec![sip_on.clone()],
        ),
        (
            "a filter whose values match nothing".to_string(),
            filters(&[("instance-id", &["i-0fedcba987654321f"])]),
            Vec::new(),
        ),
    ];
    for (_, _, expected) in &mut rows {
        expected.sort();
    }

    for (label, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the describe must succeed, got {status}: {body}"
        );
        reported = described_tasks(&body)
            .into_iter()
            .filter_map(|task| task.id)
            .collect();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {label:?}: the filter must select exactly those tasks"
        );
    }

    // The `start-time` rows above compare against the rendering a caller was shown, so every
    // task reported for one carries exactly that value.
    let (_, body) = describe(
        &router,
        &filters(&[("start-time", &[delegation_start.as_str()])]),
    )
    .await;
    for task in described_tasks(&body) {
        assert_eq!(
            task.start_time.as_deref(),
            Some(delegation_start.as_str()),
            "case \"start-time\": every task reported must carry the instant asked for"
        );
    }

    // An id this service cannot resolve is an error, not an absent row — malformed before
    // unknown — and a filter this operation does not document is refused.
    let refusals: Vec<RefusalRow> = vec![
        (
            "a task id outside the form EC2 issues".to_string(),
            vec![p("MacModificationTaskId.1", "macmdf-nope")],
            "InvalidMacModificationTaskId.Malformed",
        ),
        (
            "a task id carrying another resource's prefix".to_string(),
            vec![p("MacModificationTaskId.1", "replacevol-00000000000000001")],
            "InvalidMacModificationTaskId.Malformed",
        ),
        (
            "a well-formed task id the account holds no task for".to_string(),
            vec![p("MacModificationTaskId.1", "macmdf-00000000000000999")],
            "InvalidMacModificationTaskId.NotFound",
        ),
        (
            "a filter this operation does not document".to_string(),
            filters(&[("task-identifier", &["anything"])]),
            "InvalidParameterValue",
        ),
        (
            "the tag filter with no key at all".to_string(),
            filters(&[("tag:", &["anything"])]),
            "InvalidParameterValue",
        ),
        (
            "a token this service did not mint".to_string(),
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
    ];
    for (label, params, code) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }
}

/// Rule 6: the answer is paged in task id order, whatever page size the request asks for.
#[tokio::test]
async fn the_answer_is_paged_in_task_id_order() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let instance_id = launch(&router, apple_silicon_mac_types()[0]).await;

    let mut minted = Vec::new();
    for index in 0..5 {
        let (status, body) = create_sip(
            &router,
            &[
                p("InstanceId", instance_id.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[index % 2]),
                p("ClientToken", format!("page-{index}")),
            ],
        )
        .await;
        assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
        minted.push(created_task(&body).id.expect("the task must carry an id"));
    }
    minted.sort();

    // (the page size asked for, how many pages the walk must take)
    for (size, pages) in [(1, 5), (2, 3), (5, 1)] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut seen_pages = 0;
        loop {
            let mut params = vec![p("MaxResults", size.to_string())];
            if let Some(next) = token.clone() {
                params.push(p("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {size}\": the describe must succeed, got {status}: {body}"
            );
            let page: Vec<String> = described_tasks(&body)
                .into_iter()
                .filter_map(|task| task.id)
                .collect();
            assert!(
                page.len() <= size,
                "case \"MaxResults {size}\": a page must hold at most {size} tasks, got {page:?}"
            );
            walked.extend(page);
            seen_pages += 1;
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                seen_pages <= minted.len(),
                "case \"MaxResults {size}\": the walk must end, got {walked:?}"
            );
        }
        assert_eq!(
            walked, minted,
            "case \"MaxResults {size}\": the walk must report every task once, in id order"
        );
        assert_eq!(
            seen_pages, pages,
            "case \"MaxResults {size}\": expected {pages} pages, took {seen_pages}"
        );
    }

    // No page size at all is one page of everything.
    assert_eq!(
        all_task_ids(&router).await,
        minted,
        "case \"no page size\": the whole store must come back in id order"
    );
}

/// Rule 6: a page size outside the range the model publishes is refused, with the code EC2
/// answers for what the value is.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let instance_id = launch(&router, apple_silicon_mac_types()[0]).await;
    let (status, body) = create_sip(
        &router,
        &[
            p("InstanceId", instance_id.as_str()),
            p("MacSystemIntegrityProtectionStatus", statuses[1]),
        ],
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {status}: {body}");

    // Each published bound of this operation's own `MaxResults` range
    // (`smithy_query(query="operation",operation="DescribeMacModificationTasks")` reports
    // `range 1..500`), one value inside it, and one past each bound. Which code and sentence
    // the BOUNDARY answers is one rule for the whole service, keyed on what the value is: a
    // value that is no page size at all — zero or negative — is EC2's `InvalidParameter`
    // naming the member, the value and the bound, a POSITIVE page size below the smallest the
    // operation serves would be its `InvalidMaxResults` (this range starting at 1, there is
    // no such value here), and one above the largest keeps the boundary validator's own code.
    //
    // (what the page size is, the value, the expected status, the expected code)
    let rows: Vec<(&str, i64, u16, &str)> = vec![
        ("a page of none", 0, 400, "InvalidParameter"),
        ("a negative page", -1, 400, "InvalidParameter"),
        ("the published minimum", PUBLISHED_MIN_PAGE, 200, ""),
        (
            "inside the published range",
            (PUBLISHED_MIN_PAGE + PUBLISHED_MAX_PAGE) / 2,
            200,
            "",
        ),
        ("the published maximum", PUBLISHED_MAX_PAGE, 200, ""),
        (
            "one above the published maximum",
            PUBLISHED_MAX_PAGE + 1,
            400,
            "ValidationError",
        ),
    ];
    for (case, page_size, expected_status, code) in rows {
        let (status, body) = describe(&router, &[p("MaxResults", page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={page_size}, got \
             {status}: {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected {code} for MaxResults={page_size}, got {body}"
            );
        }
        if code == "InvalidParameter" {
            // The sentence AWS words for this error class, written out here rather than taken
            // from the implementation: its fixed text exactly, with the member as the model
            // declares it, the value the request sent and the bound it has to reach
            // interpolated from the row and the published range.
            let expected = format!(
                "value {page_size} for MaxResults is less than required minimum \
                 {PUBLISHED_MIN_PAGE}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }
}

/// Rule 7: `DryRun` says whether the call would have succeeded, and stores nothing.
#[tokio::test]
async fn dry_run_reports_whether_the_call_would_have_succeeded() {
    let router = emulator();
    let statuses = MAC_SYSTEM_INTEGRITY_PROTECTION_SETTING_STATUS_VALUES;
    let instance_id = launch(&router, apple_silicon_mac_types()[0]).await;

    // (what the dry run names, the parameters, the status and code EC2 answers)
    let rows: Vec<(String, Params, u16, &str)> = vec![
        (
            "a Mac instance the account holds".to_string(),
            vec![
                p("InstanceId", instance_id.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
        (
            "an instance id of the issued form that names nothing".to_string(),
            vec![
                p("InstanceId", "i-0fedcba987654321f"),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p("DryRun", "true"),
            ],
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "a tag specification naming another resource type".to_string(),
            vec![
                p("InstanceId", instance_id.as_str()),
                p("MacSystemIntegrityProtectionStatus", statuses[1]),
                p("TagSpecification.1.ResourceType", "instance"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
                p("DryRun", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (label, params, expected_status, code) in rows {
        let (status, body) = create_sip(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, code),
            "case {label:?}: expected {expected_status} {code}, got {status}: {body}"
        );
    }
    assert!(
        all_task_ids(&router).await.is_empty(),
        "a dry run must store no task"
    );

    // The describe answers the same probe, and stores nothing either.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "case \"a dry run of the describe\": expected 412 DryRunOperation, got {status}: {body}"
    );

    // And the request the dry run stood for is served.
    let (status, body) = create_sip(
        &router,
        &[
            p("InstanceId", instance_id.as_str()),
            p("MacSystemIntegrityProtectionStatus", statuses[1]),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the request the dry run stood for must be served, got {status}: {body}"
    );
    past_both_moves().await;
    assert_eq!(
        all_task_ids(&router).await.len(),
        1,
        "the served request must have stored exactly one task"
    );
}
