// SPDX-License-Identifier: CC0-1.0

//! EMR persistent application user interfaces, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends. Nothing
//! reaches into the service's state struct: a persistent application UI is only ever
//! read back through `DescribePersistentAppUI` and
//! `GetPersistentAppUIPresignedURL`, which is what makes these tests evidence that the
//! responses are computed from persisted state.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::MAX_PERSISTENT_APP_UIS;

use super::job_flow_lifecycle::{
    assert_minted_id, assert_refused, at, epoch_secs, kept_alive_request, Emr, REQUEST_TARGET,
};
use super::security_configurations::audit_capture;

/// The access key id this suite's audit case presents, which no other suite uses, so the
/// shared audit capture can tell its records from a sibling's.
const AUDIT_AKID: &str = "AKIDEMRPERSISTENTAPPUI";

/// Longer than the service's clock-owed transition span (500 ms), so a UI that was
/// starting has been attached by the time we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// Every value `CreatePersistentAppUI$ProfilerType` accepts, each with the
/// `PersistentAppUIType` the UI it creates is addressed by. SOURCE:
/// `smithy_query(query="shape", shape="ProfilerType,PersistentAppUIType")` —
/// `ProfilerType` is `SHS | TEZUI | YTS` and `PersistentAppUIType` is `SHS | TEZ | YTS`.
const PROFILER_TO_UI_TYPE: [(&str, &str); 3] = [("SHS", "SHS"), ("TEZUI", "TEZ"), ("YTS", "YTS")];

/// The UI a create that names no profiler publishes.
const DEFAULT_UI_TYPE: &str = "SHS";

/// The interface a presigned URL opens, without the session token the answering call
/// minted into it: everything before the query string.
///
/// `pub(crate)` so the suite that spans the per-call freshness of that token reads one URL
/// the same way.
pub(crate) fn opened_interface(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// The status a freshly created UI carries, and the one the clock owes it.
const STARTING: &str = "STARTING";
const ATTACHED: &str = "ATTACHED";

/// The persistent-application-UI calls, over the shared [`Emr`] harness rather than a
/// copy of it.
struct PersistentAppUis {
    emr: Emr,
}

impl PersistentAppUis {
    fn new() -> Self {
        Self { emr: Emr::new() }
    }

    /// A cluster this account holds, answering the ARN `RunJobFlow` minted for it —
    /// which is what a `TargetResourceArn` naming an EMR on EC2 cluster carries.
    async fn cluster_arn(&self, name: &str, request: Value) -> String {
        let out = self.emr.ok("RunJobFlow", request).await;
        out["ClusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: RunJobFlow must answer a ClusterArn, got {out}"))
            .to_string()
    }

    async fn create(&self, request: Value) -> (u16, Value) {
        self.emr.call("CreatePersistentAppUI", request).await
    }

    /// `CreatePersistentAppUI`, answering the minted UI id.
    async fn create_ok(&self, case: &str, request: Value) -> String {
        let out = self.emr.ok("CreatePersistentAppUI", request).await;
        out["PersistentAppUIId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: CreatePersistentAppUI must answer a PersistentAppUIId, got {out}")
            })
            .to_string()
    }

    async fn describe(&self, id: &str) -> (u16, Value) {
        self.emr
            .call("DescribePersistentAppUI", json!({ "PersistentAppUIId": id }))
            .await
    }

    async fn describe_ok(&self, id: &str) -> Value {
        self.emr
            .ok("DescribePersistentAppUI", json!({ "PersistentAppUIId": id }))
            .await["PersistentAppUI"]
            .clone()
    }

    async fn presigned(&self, request: Value) -> (u16, Value) {
        self.emr
            .call("GetPersistentAppUIPresignedURL", request)
            .await
    }

    async fn presigned_ok(&self, request: Value) -> Value {
        self.emr.ok("GetPersistentAppUIPresignedURL", request).await
    }
}

/// A security configuration document whose `AuthorizationConfiguration` switches
/// application-scoped IAM roles (runtime roles) on or off, or leaves the section out
/// entirely. (Amazon EMR Management Guide, "Runtime roles for Amazon EMR steps".)
fn runtime_role_document(enabled: Option<bool>) -> String {
    match enabled {
        None => json!({ "EncryptionConfiguration": { "EnableInTransitEncryption": false } }).to_string(),
        Some(enabled) => json!({
            "AuthorizationConfiguration": {
                "IAMConfiguration": { "EnableApplicationScopedIAMRole": enabled },
            },
        })
        .to_string(),
    }
}

/// The string list at `path`, for comparing a `PersistentAppUITypeList` with what the
/// create asked for.
fn string_list(case: &str, payload: &Value, path: &[&str]) -> Vec<String> {
    at(payload, path)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list at {path:?} in {payload}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected strings at {path:?} in {payload}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CreatePersistentAppUI -> DescribePersistentAppUI
// ---------------------------------------------------------------------------

/// THE RULE: `CreatePersistentAppUI` persists a UI built from the request under a minted
/// id and `DescribePersistentAppUI` answers that SAME UI back — the type list the
/// requested profiler publishes, the tags as supplied, the instant it was created, and
/// the starting status it has not left yet. The table spans every `ProfilerType` the
/// model declares plus the absent one, each target service EMR publishes a UI for, and
/// tag lists of three sizes.
#[tokio::test]
async fn create_persistent_app_ui_persists_each_ui_and_describe_reads_it_back() {
    struct Case {
        label: &'static str,
        request: Value,
        ui_types: Vec<&'static str>,
        tags: Vec<Value>,
    }

    let uis = PersistentAppUis::new();
    let cluster_arn = uis
        .cluster_arn("target cluster", kept_alive_request("ui-target"))
        .await;

    let mut cases = vec![
        Case {
            label: "cluster target, no profiler, no tags",
            request: json!({ "TargetResourceArn": cluster_arn }),
            ui_types: vec![DEFAULT_UI_TYPE],
            tags: vec![],
        },
        Case {
            label: "EMR on EKS job run target, one tag",
            request: json!({
                "TargetResourceArn":
                    "arn:aws:emr-containers:us-east-1:123456789012:/virtualclusters/0abcdefghijkl",
                "EMRContainersConfig": { "JobRunId": "00000003abcdefghij" },
                "Tags": [{ "Key": "team", "Value": "analytics" }],
            }),
            ui_types: vec![DEFAULT_UI_TYPE],
            tags: vec![json!({ "Key": "team", "Value": "analytics" })],
        },
        Case {
            label: "EMR Serverless target, three tags and a cross reference",
            request: json!({
                "TargetResourceArn":
                    "arn:aws:emr-serverless:eu-west-1:123456789012:/applications/00f1abcdefghijkl",
                "XReferer": "https://console.aws.amazon.com/emr",
                "Tags": [
                    { "Key": "a", "Value": "1" },
                    { "Key": "b", "Value": "2" },
                    { "Key": "c", "Value": "" },
                ],
            }),
            ui_types: vec![DEFAULT_UI_TYPE],
            tags: vec![
                json!({ "Key": "a", "Value": "1" }),
                json!({ "Key": "b", "Value": "2" }),
                json!({ "Key": "c", "Value": "" }),
            ],
        },
    ];
    // One row per profiler type the model declares, each on the cluster target.
    for (profiler, ui_type) in PROFILER_TO_UI_TYPE {
        cases.push(Case {
            label: profiler,
            request: json!({ "TargetResourceArn": cluster_arn, "ProfilerType": profiler }),
            ui_types: vec![ui_type],
            tags: vec![],
        });
    }

    for case in &cases {
        let before = epoch_secs(SystemTime::now());
        let id = uis.create_ok(case.label, case.request.clone()).await;
        assert_minted_id(case.label, "p-", &id);

        let ui = uis.describe_ok(&id).await;
        assert_eq!(
            at(&ui, &["PersistentAppUIId"]).as_str(),
            Some(id.as_str()),
            "case {:?}: Describe must answer the UI the create minted, got {ui}",
            case.label
        );
        assert_eq!(
            string_list(case.label, &ui, &["PersistentAppUITypeList"]),
            case.ui_types,
            "case {:?}: unexpected PersistentAppUITypeList in {ui}",
            case.label
        );
        assert_eq!(
            at(&ui, &["PersistentAppUIStatus"]).as_str(),
            Some(STARTING),
            "case {:?}: a UI that was just created has not been attached yet, got {ui}",
            case.label
        );
        assert!(
            at(&ui, &["LastStateChangeReason"]).is_null(),
            "case {:?}: a UI that never changed state carries no reason, got {ui}",
            case.label
        );
        assert!(
            at(&ui, &["AuthorId"])
                .as_str()
                .is_some_and(|author| author.starts_with("arn:")),
            "case {:?}: expected the calling principal as AuthorId, got {ui}",
            case.label
        );
        let created = at(&ui, &["CreationTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no CreationTime in {ui}", case.label));
        assert!(
            created >= before && created <= epoch_secs(SystemTime::now()),
            "case {:?}: CreationTime {created} must be the instant of the create, got {ui}",
            case.label
        );
        let tags = at(&ui, &["Tags"]);
        if case.tags.is_empty() {
            assert!(
                tags.is_null(),
                "case {:?}: a UI with no tags reports none, got {ui}",
                case.label
            );
        } else {
            assert_eq!(
                tags,
                &Value::Array(case.tags.clone()),
                "case {:?}: tags must round trip as supplied, got {ui}",
                case.label
            );
        }

        // A read changes nothing a later caller sees: the id, the type list, the
        // author, the creation instant and the tags are the same on a second read.
        // (The status is left out here because the clock may make the attach due
        // between the two reads; the clock-owed test below pins both statuses.)
        let again = uis.describe_ok(&id).await;
        for member in [
            "PersistentAppUIId",
            "PersistentAppUITypeList",
            "AuthorId",
            "CreationTime",
            "Tags",
        ] {
            assert_eq!(
                at(&again, &[member]),
                at(&ui, &[member]),
                "case {:?}: two DescribePersistentAppUI calls must report the same {member}, \
                 got {again} after {ui}",
                case.label
            );
        }
    }

    // Every create minted its own id, so no two UIs share one.
    let mut ids = Vec::new();
    for case in &cases {
        ids.push(uis.create_ok(case.label, case.request.clone()).await);
    }
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "each CreatePersistentAppUI mints its own id, got {ids:?}"
    );
}

/// THE RULE: `RuntimeRoleEnabledCluster` is computed from the TARGET cluster's security
/// configuration — true only where that configuration's `AuthorizationConfiguration`
/// switches application-scoped IAM roles on. The rows span a configuration that enables
/// them, one that disables them, one that carries no authorization section, a cluster
/// naming no configuration at all, and a cluster naming a configuration this account
/// does not hold.
#[tokio::test]
async fn runtime_role_enabled_cluster_follows_the_target_clusters_security_configuration() {
    struct Case {
        label: &'static str,
        configuration: Option<(&'static str, Option<String>)>,
        expected: bool,
    }

    let uis = PersistentAppUis::new();
    let cases = [
        Case {
            label: "EnableApplicationScopedIAMRole true",
            configuration: Some(("runtime-roles-on", Some(runtime_role_document(Some(true))))),
            expected: true,
        },
        Case {
            label: "EnableApplicationScopedIAMRole false",
            configuration: Some(("runtime-roles-off", Some(runtime_role_document(Some(false))))),
            expected: false,
        },
        Case {
            label: "no AuthorizationConfiguration section",
            configuration: Some(("encryption-only", Some(runtime_role_document(None)))),
            expected: false,
        },
        Case {
            label: "a security configuration this account does not hold",
            configuration: Some(("never-created", None)),
            expected: false,
        },
        Case {
            label: "no security configuration",
            configuration: None,
            expected: false,
        },
    ];

    for case in &cases {
        let mut request = kept_alive_request(case.label);
        if let Some((name, document)) = &case.configuration {
            if let Some(document) = document {
                uis.emr
                    .ok(
                        "CreateSecurityConfiguration",
                        json!({ "Name": name, "SecurityConfiguration": document }),
                    )
                    .await;
            }
            request["SecurityConfiguration"] = json!(name);
        }
        let cluster_arn = uis.cluster_arn(case.label, request).await;

        let out = uis
            .emr
            .ok(
                "CreatePersistentAppUI",
                json!({ "TargetResourceArn": cluster_arn }),
            )
            .await;
        assert_eq!(
            at(&out, &["RuntimeRoleEnabledCluster"]).as_bool(),
            Some(case.expected),
            "case {:?}: expected RuntimeRoleEnabledCluster {}, got {out}",
            case.label,
            case.expected
        );
    }

    // A target that is no EMR on EC2 cluster has no such cluster, so it is not one.
    let out = uis
        .emr
        .ok(
            "CreatePersistentAppUI",
            json!({
                "TargetResourceArn":
                    "arn:aws:emr-serverless:us-east-1:123456789012:/applications/00f1abcdefghijkl",
            }),
        )
        .await;
    assert_eq!(
        at(&out, &["RuntimeRoleEnabledCluster"]).as_bool(),
        Some(false),
        "case \"EMR Serverless target\": a non-cluster target is not runtime role enabled, got {out}"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed attach, and the presigned URL that follows it
// ---------------------------------------------------------------------------

/// THE RULE: a created UI is not serving yet, so `GetPersistentAppUIPresignedURL`
/// answers `PresignedURLReady: false` and no URL; once the clock makes the attach due
/// the UI reports the attached status and the call answers a URL derived from that UI —
/// the same URL on every later read. The rows span every UI type a create can publish.
#[tokio::test]
async fn a_starting_ui_is_attached_when_the_clock_makes_it_due_and_its_url_follows() {
    let uis = PersistentAppUis::new();
    let cluster_arn = uis
        .cluster_arn("attach target", kept_alive_request("attaching"))
        .await;

    let mut created = Vec::new();
    for (profiler, ui_type) in PROFILER_TO_UI_TYPE {
        let id = uis
            .create_ok(
                profiler,
                json!({ "TargetResourceArn": cluster_arn, "ProfilerType": profiler }),
            )
            .await;
        let out = uis
            .presigned_ok(json!({ "PersistentAppUIId": id }))
            .await;
        assert_eq!(
            at(&out, &["PresignedURLReady"]).as_bool(),
            Some(false),
            "case {profiler:?}: a UI that is still starting has no URL yet, got {out}"
        );
        assert!(
            at(&out, &["PresignedURL"]).is_null(),
            "case {profiler:?}: no URL is answered before the UI is attached, got {out}"
        );
        created.push((profiler, ui_type, id));
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    for (profiler, ui_type, id) in &created {
        let ui = uis.describe_ok(id).await;
        assert_eq!(
            at(&ui, &["PersistentAppUIStatus"]).as_str(),
            Some(ATTACHED),
            "case {profiler:?}: expected the attached status once it is due, got {ui}"
        );
        assert!(
            at(&ui, &["LastStateChangeReason"])
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {profiler:?}: the state change carries its reason, got {ui}"
        );
        let created_at = at(&ui, &["CreationTime"]).as_f64().unwrap_or_default();
        let modified = at(&ui, &["LastModifiedTime"])
            .as_f64()
            .unwrap_or_else(|| panic!("case {profiler:?}: no LastModifiedTime in {ui}"));
        assert!(
            modified > created_at,
            "case {profiler:?}: LastModifiedTime {modified} must follow CreationTime {created_at}"
        );

        let out = uis.presigned_ok(json!({ "PersistentAppUIId": id })).await;
        assert_eq!(
            at(&out, &["PresignedURLReady"]).as_bool(),
            Some(true),
            "case {profiler:?}: an attached UI is ready, got {out}"
        );
        let url = at(&out, &["PresignedURL"])
            .as_str()
            .unwrap_or_else(|| panic!("case {profiler:?}: no PresignedURL in {out}"))
            .to_string();
        assert!(
            url.starts_with("https://") && url.contains(id.as_str()),
            "case {profiler:?}: the URL must be derived from the UI, got {url}"
        );
        assert!(
            url.contains(&ui_type.to_lowercase()),
            "case {profiler:?}: the URL must name the {ui_type} interface, got {url}"
        );

        // Asking for the type the UI publishes, by name, opens the same interface — and
        // the URL is presigned afresh, so the token in it is not the first call's.
        let named = uis
            .presigned_ok(json!({ "PersistentAppUIId": id, "PersistentAppUIType": ui_type }))
            .await;
        let named_url = at(&named, &["PresignedURL"])
            .as_str()
            .unwrap_or_else(|| panic!("case {profiler:?}: no PresignedURL in {named}"))
            .to_string();
        assert_eq!(
            opened_interface(&named_url),
            opened_interface(&url),
            "case {profiler:?}: naming the published type opens the same interface, got {named}"
        );
        assert_ne!(
            named_url, url,
            "case {profiler:?}: each call presigns the URL afresh, got {named_url} twice"
        );

        // "The application ID associated with the presigned URL": it selects which
        // application's interface the URL opens, so it is part of that URL.
        let for_application = uis
            .presigned_ok(json!({
                "PersistentAppUIId": id,
                "ApplicationId": "application_1700000000000_0001",
            }))
            .await;
        let application_url = at(&for_application, &["PresignedURL"])
            .as_str()
            .unwrap_or_else(|| panic!("case {profiler:?}: no PresignedURL in {for_application}"));
        assert!(
            application_url.starts_with(opened_interface(&url))
                && application_url.contains("application_1700000000000_0001"),
            "case {profiler:?}: expected the application's own URL under {url}, got {application_url}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// THE RULE: a `TargetResourceArn` that is not an ARN of a resource Amazon EMR
/// publishes an application user interface for is refused with HTTP 400
/// `ValidationException`, and the refused create stores NOTHING. The rows span each
/// ARN field a malformed value can break — the literal, the partition, the service, the
/// field count and the resource — plus a well-formed ARN of another service and a
/// cluster ARN this account holds no cluster for.
#[tokio::test]
async fn a_target_resource_that_names_no_emr_resource_is_refused_and_stores_nothing() {
    let uis = PersistentAppUis::new();
    let cluster_arn = uis
        .cluster_arn("refusal target", kept_alive_request("refusals"))
        .await;

    // Each row is at least 20 characters long, which is `ArnType`'s documented minimum
    // length, so every one of them reaches the handler rather than the boundary
    // validator.
    let malformed = [
        ("no ARN at all", "a-target-resource-that-is-not-an-arn".to_string()),
        (
            "the arn literal misspelled",
            "urn:aws:elasticmapreduce:us-east-1:123456789012:cluster/j-ABCDEFGHIJKLM".to_string(),
        ),
        (
            "too few fields",
            "arn:aws:elasticmapreduce:us-east-1:123456789012".to_string(),
        ),
        (
            "no partition",
            "arn::elasticmapreduce:us-east-1:123456789012:cluster/j-ABCDEFGHIJKLM".to_string(),
        ),
        (
            "no service",
            "arn:aws::us-east-1:123456789012:cluster/j-ABCDEFGHIJKLM".to_string(),
        ),
        (
            "no resource",
            "arn:aws:elasticmapreduce:us-east-1:123456789012:".to_string(),
        ),
        (
            "a service EMR publishes no application UI for",
            "arn:aws:s3:::a-bucket-of-event-logs".to_string(),
        ),
        (
            "a cluster this account does not hold",
            "arn:aws:elasticmapreduce:us-east-1:123456789012:cluster/j-NOSUCHCLUSTER".to_string(),
        ),
        (
            "an EMR resource that is no cluster",
            "arn:aws:elasticmapreduce:us-east-1:123456789012:studio/es-ABCDEFGHIJKLM".to_string(),
        ),
        (
            "an EMR ARN naming a bare resource type",
            "arn:aws:elasticmapreduce:us-east-1:123456789012:cluster".to_string(),
        ),
        (
            "an EMR ARN naming an empty cluster id",
            "arn:aws:elasticmapreduce:us-east-1:123456789012:cluster/".to_string(),
        ),
    ];

    for (label, target) in &malformed {
        let (status, payload) = uis
            .create(json!({ "TargetResourceArn": target }))
            .await;
        assert_refused(label, status, &payload, "ValidationException");
    }

    // A profiler type the model does not declare is refused by the modelled enum
    // constraint at the boundary, before the handler runs — and this service words an
    // enum-value refusal as its own modelled `InvalidRequestException` (see
    // `EmulatedService::override_refusal`), not as the generic validator's error.
    let (status, payload) = uis
        .create(json!({ "TargetResourceArn": cluster_arn, "ProfilerType": "SPARKUI" }))
        .await;
    assert_refused(
        "an undeclared ProfilerType",
        status,
        &payload,
        "InvalidRequestException",
    );

    // A value shorter than `ArnType`'s documented minimum of 20 characters never
    // reaches the handler: the modelled constraint refuses it at the boundary. This row
    // pins the status rather than the handler's own error channel.
    let (status, payload) = uis.create(json!({ "TargetResourceArn": "arn:aws:s3:::x" })).await;
    assert_eq!(
        status, 400,
        "case \"shorter than ArnType's minimum\": expected HTTP 400, got {status} with {payload}"
    );

    // Not one of the refusals above stored anything: the first UI this account holds is
    // the one the next ACCEPTED create mints, which is the id a store that saw only that
    // one create mints too.
    let after_refusals = uis
        .create_ok("after the refusals", json!({ "TargetResourceArn": cluster_arn }))
        .await;
    let fresh = PersistentAppUis::new();
    let fresh_cluster = fresh
        .cluster_arn("fresh target", kept_alive_request("fresh"))
        .await;
    let first_ever = fresh
        .create_ok("first create of a fresh store", json!({ "TargetResourceArn": fresh_cluster }))
        .await;
    assert_eq!(
        after_refusals, first_ever,
        "a refused create must mint no id: expected {first_ever}, got {after_refusals}"
    );
}

/// THE RULE: the account holds a bounded number of persistent application user
/// interfaces, so a create past that bound is refused rather than accepted into
/// unbounded growth — probed at the bound and one past it.
#[tokio::test]
async fn the_account_quota_is_enforced_at_its_bound() {
    let uis = PersistentAppUis::new();
    let cluster_arn = uis
        .cluster_arn("quota target", kept_alive_request("quota"))
        .await;

    // Up to and including the bound every create is accepted.
    for held in 0..MAX_PERSISTENT_APP_UIS {
        let (status, payload) = uis
            .create(json!({ "TargetResourceArn": cluster_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case \"UI {} of {MAX_PERSISTENT_APP_UIS}\": expected 200, got {status} with {payload}",
            held + 1
        );
    }

    // One past it is refused, and the refusal leaves the store as it was.
    let (status, payload) = uis
        .create(json!({ "TargetResourceArn": cluster_arn }))
        .await;
    assert_refused(
        "one past the account quota",
        status,
        &payload,
        "ValidationException",
    );
}

/// THE RULE: an id no persistent application UI carries is refused with HTTP 400 by
/// both readers — never answered empty — and so is a `PersistentAppUIType` the UI does
/// not publish and an `ExecutionRoleArn` that is not an ARN. The rows span the id forms
/// a caller can get wrong: another UI's shape, a cluster id, an empty id and a
/// stale-looking one. The two readers answer different error codes, as the service
/// does: `DescribePersistentAppUI` its own modelled `InvalidRequestException`,
/// `GetPersistentAppUIPresignedURL` a `ValidationException`.
#[tokio::test]
async fn unknown_persistent_app_ui_ids_and_types_are_refused_rather_than_answered() {
    let uis = PersistentAppUis::new();
    let cluster_arn = uis
        .cluster_arn("reader target", kept_alive_request("readers"))
        .await;
    let id = uis
        .create_ok(
            "the one UI this account holds",
            json!({ "TargetResourceArn": cluster_arn, "ProfilerType": "SHS" }),
        )
        .await;

    for unknown in [
        "p-NOSUCHUI00000",
        "j-ABCDEFGHIJKLM",
        "",
        "p-0000000000000",
        &format!("{id}x"),
    ] {
        let label = format!("DescribePersistentAppUI {unknown:?}");
        let (status, payload) = uis.describe(unknown).await;
        assert_refused(&label, status, &payload, "InvalidRequestException");

        let label = format!("GetPersistentAppUIPresignedURL {unknown:?}");
        let (status, payload) = uis
            .presigned(json!({ "PersistentAppUIId": unknown }))
            .await;
        assert_refused(&label, status, &payload, "ValidationException");
    }

    // The UI publishes SHS, so the other two types it does not publish are refused.
    for ui_type in ["TEZ", "YTS"] {
        let label = format!("PersistentAppUIType {ui_type}");
        let (status, payload) = uis
            .presigned(json!({ "PersistentAppUIId": id, "PersistentAppUIType": ui_type }))
            .await;
        assert_refused(&label, status, &payload, "ValidationException");
    }

    // An `ExecutionRoleArn` that names no IAM role of this account names no role to
    // assume. Rows: not an ARN, too few fields, no partition, another IAM entity type,
    // another vendor, another account's role, a Region an IAM ARN does not carry, and an
    // empty role name.
    let own_account = engine::emulator_account_id();
    for role_arn in [
        "a-role-that-is-not-an-arn-at-all".to_string(),
        "arn:aws:iam::123456789012".to_string(),
        format!("arn::iam::{own_account}:role/EMRExecution"),
        format!("arn:aws:iam::{own_account}:user/analyst"),
        "arn:aws:s3:::a-bucket-of-event-logs".to_string(),
        "arn:aws:iam::123456789012:role/EMRExecution".to_string(),
        format!("arn:aws:iam:us-east-1:{own_account}:role/EMRExecution"),
        format!("arn:aws:iam::{own_account}:role/"),
    ] {
        let label = format!("ExecutionRoleArn {role_arn:?}");
        let (status, payload) = uis
            .presigned(json!({ "PersistentAppUIId": id, "ExecutionRoleArn": role_arn }))
            .await;
        assert_refused(&label, status, &payload, "ValidationException");
    }

    // A role of this account IS accepted, under the default path, under a path and in
    // another partition (an IAM ARN is partition-global, so it carries no Region), and
    // the authentication-proxy flag changes nothing about which UI is addressed: the UI
    // is still starting, so the answer is the one its own readiness gives.
    for role_arn in [
        format!("arn:aws:iam::{own_account}:role/EMRExecution"),
        format!("arn:aws:iam::{own_account}:role/service-role/EMRExecution"),
        format!("arn:aws-cn:iam::{own_account}:role/EMRExecution"),
    ] {
        let out = uis
            .presigned_ok(json!({
                "PersistentAppUIId": id,
                "ExecutionRoleArn": role_arn,
                "AuthProxyCall": true,
            }))
            .await;
        assert_eq!(
            at(&out, &["PresignedURLReady"]).as_bool(),
            Some(false),
            "case {role_arn:?}: the UI is still starting, so no URL is ready yet, got {out}"
        );
        assert!(
            at(&out, &["PresignedURL"]).is_null(),
            "case {role_arn:?}: no URL is answered before the UI is attached, got {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// The access record a state-changing call leaves
// ---------------------------------------------------------------------------

/// THE RULE: minting a user interface CHANGES this service's state, so
/// `CreatePersistentAppUI` leaves exactly ONE structured audit record — on its refusal
/// branches too — naming the operation, the resource (the id it minted, else the target
/// the request asked for), the presented access key id, the request target and the
/// outcome, and carrying no part of the request's own data. The two readers only report,
/// so they leave none.
#[tokio::test]
async fn creating_a_ui_emits_one_audit_record_and_reading_one_emits_none() {
    let capture = audit_capture();
    let emr = Emr::with_akid(AUDIT_AKID);

    // Harness self-test, so a failure below can never be mistaken for a missing record.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = capture.drain_audit_records(AUDIT_AKID);
    assert_eq!(
        probe.len(),
        1,
        "the capture harness must see an audit record emitted on this thread, got {probe:?}"
    );

    let launched = emr.ok("RunJobFlow", kept_alive_request("audited-ui")).await;
    let cluster_arn = launched["ClusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("RunJobFlow must answer a ClusterArn, got {launched}"))
        .to_string();
    // The launch leaves its own record; this case is about the create's.
    let _ = capture.drain_audit_records(AUDIT_AKID);

    // A value that must never reach the log, carried in the part of the request that
    // describes the UI rather than names its target.
    let secret = "not-in-the-log";
    let unknown_cluster_arn =
        "arn:aws:elasticmapreduce:us-east-1:123456789012:cluster/j-NOSUCHCLUSTER";

    // (label, request, the outcome it must record, the resource it must name — `None`
    // means "the id the create minted").
    let audited = [
        (
            "a create that persists a UI",
            json!({
                "TargetResourceArn": cluster_arn,
                "Tags": [{ "Key": "owner", "Value": secret }],
            }),
            "persistent-app-ui-created",
            None,
        ),
        (
            "a create whose target is no ARN at all",
            json!({ "TargetResourceArn": "a-target-resource-that-is-not-an-arn" }),
            "validation-refused",
            Some("a-target-resource-that-is-not-an-arn"),
        ),
        (
            "a create naming a cluster this account does not hold",
            json!({ "TargetResourceArn": unknown_cluster_arn }),
            "validation-refused",
            Some(unknown_cluster_arn),
        ),
    ];

    let mut created = String::new();
    for (label, request, outcome, expected_resource) in audited {
        let (_, payload) = emr.call("CreatePersistentAppUI", request).await;
        let records = capture.drain_audit_records(AUDIT_AKID);
        assert_eq!(
            records.len(),
            1,
            "case {label:?}: expected exactly one audit record, captured {records:?}"
        );
        let record = &records[0];
        let resource = match expected_resource {
            Some(resource) => resource.to_string(),
            None => {
                created = payload["PersistentAppUIId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: no PersistentAppUIId in {payload}")
                    })
                    .to_string();
                created.clone()
            }
        };
        for (field, expected) in [
            ("operation", "CreatePersistentAppUI"),
            ("resource", resource.as_str()),
            ("outcome", outcome),
            ("access_key_id", AUDIT_AKID),
            ("request_target", REQUEST_TARGET),
        ] {
            assert_eq!(
                record.field(field),
                Some(expected),
                "case {label:?}: expected {field}={expected:?}, got {record:?}"
            );
        }
        // Metadata only: no part of the submitted request's own data is logged.
        let rendered = format!("{record:?}");
        assert!(
            !rendered.contains(secret),
            "case {label:?}: the record must carry no part of the request's data, got {rendered}"
        );
    }

    // Reading a UI records nothing: there is no state change to attribute, on the
    // answered path or the refused one.
    for (operation, request) in [
        ("DescribePersistentAppUI", json!({ "PersistentAppUIId": &created })),
        ("GetPersistentAppUIPresignedURL", json!({ "PersistentAppUIId": &created })),
        ("DescribePersistentAppUI", json!({ "PersistentAppUIId": "p-NOSUCHUI00000" })),
        (
            "GetPersistentAppUIPresignedURL",
            json!({ "PersistentAppUIId": "p-NOSUCHUI00000" }),
        ),
    ] {
        let _ = emr.call(operation, request).await;
        let leaked = capture.drain_audit_records(AUDIT_AKID);
        assert!(
            leaked.is_empty(),
            "case {operation:?}: a reporting operation must emit no audit record, \
             captured {leaked:?}"
        );
    }
}
