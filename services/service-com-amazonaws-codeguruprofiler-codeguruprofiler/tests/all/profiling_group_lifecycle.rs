// SPDX-License-Identifier: CC0-1.0

//! The profiling-group lifecycle and the agent-profile round trip, end to end over
//! HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: each profiling-group operation answers from the
//! ONE stored group record — create/configure/post write it, describe/list/get read it
//! back, delete removes it — so no response is an echo of the request that asked for
//! it. Each table row varies what the rule generalises over (compute platform,
//! orchestration flag, tag count, name length, payload bytes, media type, page size);
//! no row is privileged.

use std::collections::BTreeMap;
use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this
/// service by its SigV4 credential scope, the way an SDK request does. Signature
/// verification is off in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for; every ARN this service mints
/// for these requests carries it. `pub(crate)` so every suite in this test target
/// names the emulated region once.
pub(crate) const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The ARN this emulator mints for a profiling group of this name, in the region and
/// account the harness's requests are signed for — the one source of that form for
/// every suite in this test target.
pub(crate) fn group_arn(name: &str) -> String {
    format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/{name}")
}

/// One HTTP answer, reduced to what these tests assert on. `pub(crate)` so the other
/// suites in this test target drive the same harness instead of forking it.
pub(crate) struct Answer {
    pub(crate) status: u16,
    content_type: Option<String>,
    pub(crate) body: Vec<u8>,
}

impl Answer {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    pub(crate) fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine
/// behind one mounted service. The engine is kept so a test can assert on the stored
/// state itself, which is the only way to observe a retention bound.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    /// `pub(crate)` for the same reason as the harness itself: a sibling suite that
    /// has to place a stored record at a chosen instant reaches the one shared engine
    /// through it instead of standing up a second emulator.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The shared engine behind the mounted service, so another suite in this target
    /// can seed or inspect the stored state the way the tests below do.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// What one group holds right now: (retained profiles, retained profile bytes,
    /// remembered profile tokens).
    fn retained(&self, name: &str) -> (usize, usize, usize) {
        let state = self
            .engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
        let guard = state.lock();
        let group = guard
            .groups
            .get(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"));
        (
            group.profiles.len(),
            group.profiles.iter().map(|p| p.bytes.len()).sum(),
            group.seen_profile_tokens.len(),
        )
    }

    /// The revision of one group's STORED resource-based policy, or `None` when the
    /// stored group holds none — the only way to tell a policy that was deleted from
    /// one that merely reads back as granting nothing.
    pub(crate) fn stored_policy_revision(&self, name: &str) -> Option<String> {
        let state = self
            .engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
        let guard = state.lock();
        guard
            .groups
            .get(name)
            .unwrap_or_else(|| panic!("no stored group named {name}"))
            .policy
            .as_ref()
            .map(|policy| policy.revision_id.clone())
    }

    /// Profile bytes retained across every group — what the store-wide budget bounds.
    fn retained_profile_bytes(&self) -> usize {
        self.engine
            .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
            .lock()
            .retained_profile_bytes()
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        uri: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", content_type)
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status,
            content_type,
            body: response.into_body().to_vec(),
        }
    }

    pub(crate) async fn create(&self, case: &GroupCase, client_token: &str) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups?clientToken={client_token}"),
            "application/json",
            serde_json::to_vec(&case.request_body()).expect("a serializable body"),
        )
        .await
    }

    pub(crate) async fn describe(&self, name: &str) -> Answer {
        self.send(
            "GET",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
    }

    async fn list(&self, query: &str) -> Answer {
        self.send(
            "GET",
            &format!("/profilingGroups{query}"),
            "application/json",
            Vec::new(),
        )
        .await
    }

    pub(crate) async fn configure_agent(&self, name: &str) -> Answer {
        self.send(
            "POST",
            &format!("/profilingGroups/{name}/configureAgent"),
            "application/json",
            b"{}".to_vec(),
        )
        .await
    }

    pub(crate) async fn post_agent_profile(
        &self,
        name: &str,
        content_type: &str,
        token: Option<&str>,
        bytes: Vec<u8>,
    ) -> Answer {
        let query = token.map(|t| format!("?profileToken={t}")).unwrap_or_default();
        self.send(
            "POST",
            &format!("/profilingGroups/{name}/agentProfile{query}"),
            content_type,
            bytes,
        )
        .await
    }

    pub(crate) async fn get_profile(&self, name: &str, query: &str) -> Answer {
        self.get_profile_accepting(name, query, None).await
    }

    /// `GetProfile` with the modelled `accept` member, which binds to the `Accept`
    /// request header rather than to the query string.
    async fn get_profile_accepting(
        &self,
        name: &str,
        query: &str,
        accept: Option<&str>,
    ) -> Answer {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(format!("/profilingGroups/{name}/profile{query}"))
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION);
        if let Some(accept) = accept {
            request = request.header("accept", accept);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::new())
                    .expect("a well-formed request"),
            )
            .await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status,
            content_type,
            body: response.into_body().to_vec(),
        }
    }

    async fn delete(&self, name: &str) -> Answer {
        self.send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
    }
}

/// One `CreateProfilingGroup` input, plus what the stored group must then report.
#[derive(Debug)]
pub(crate) struct GroupCase {
    pub(crate) name: String,
    /// `computePlatform` as sent; `None` exercises the model's documented default.
    compute_platform: Option<&'static str>,
    /// `agentOrchestrationConfig.profilingEnabled` as sent; `None` omits the config.
    profiling_enabled: Option<bool>,
    pub(crate) tags: BTreeMap<&'static str, &'static str>,
}

impl GroupCase {
    fn request_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert(
            "profilingGroupName".to_string(),
            serde_json::Value::from(self.name.clone()),
        );
        if let Some(platform) = self.compute_platform {
            body.insert(
                "computePlatform".to_string(),
                serde_json::Value::from(platform),
            );
        }
        if let Some(enabled) = self.profiling_enabled {
            body.insert(
                "agentOrchestrationConfig".to_string(),
                serde_json::json!({ "profilingEnabled": enabled }),
            );
        }
        if !self.tags.is_empty() {
            body.insert(
                "tags".to_string(),
                serde_json::Value::Object(
                    self.tags
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), serde_json::Value::from(*v)))
                        .collect(),
                ),
            );
        }
        serde_json::Value::Object(body)
    }

    /// `computePlatform` the group must report: the model's doc for that member says
    /// "If not specified, Default is used."
    pub(crate) fn expected_compute_platform(&self) -> &str {
        self.compute_platform.unwrap_or("Default")
    }

    /// `profilingEnabled` the group must report: an absent `agentOrchestrationConfig`
    /// leaves the agents orchestrated, i.e. profiling enabled.
    pub(crate) fn expected_profiling_enabled(&self) -> bool {
        self.profiling_enabled.unwrap_or(true)
    }

    pub(crate) fn expected_arn(&self) -> String {
        group_arn(&self.name)
    }
}

/// The table: rows differ in compute platform, orchestration flag, tag count and name
/// length (1 and 255 are the `ProfilingGroupName` length bounds the model declares).
pub(crate) fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            name: "demo-app".to_string(),
            compute_platform: None,
            profiling_enabled: Some(true),
            tags: BTreeMap::from([("env", "dev")]),
        },
        GroupCase {
            name: "lambda_handler-9".to_string(),
            compute_platform: Some("AWSLambda"),
            profiling_enabled: Some(false),
            tags: BTreeMap::new(),
        },
        GroupCase {
            name: "Mixed_Case-Group".to_string(),
            compute_platform: Some("Default"),
            profiling_enabled: None,
            tags: BTreeMap::from([("team", "payments"), ("tier", "prod"), ("zz", "")]),
        },
        GroupCase {
            name: "x".to_string(),
            compute_platform: Some("AWSLambda"),
            profiling_enabled: None,
            tags: BTreeMap::new(),
        },
        GroupCase {
            name: "g".repeat(255),
            compute_platform: Some("Default"),
            profiling_enabled: Some(false),
            tags: BTreeMap::from([("only", "one")]),
        },
    ]
}

/// Every member of a group's description, per the case that created it. `label` names the
/// path the description came from, and `reports_profiling_status` says whether that path
/// carries the `profilingStatus` container: a READ of a stored group does (empty while
/// nothing has been reported about it), while the CREATE's own payload omits it, having
/// nothing to report about a group that is only now coming into existence.
fn assert_description(
    case: &GroupCase,
    label: &str,
    reports_profiling_status: bool,
    description: &serde_json::Value,
) {
    assert_eq!(
        description.get("name").and_then(serde_json::Value::as_str),
        Some(case.name.as_str()),
        "case {case:?} ({label}): wrong name, got {description:?}"
    );
    assert_eq!(
        description.get("arn").and_then(serde_json::Value::as_str),
        Some(case.expected_arn().as_str()),
        "case {case:?} ({label}): wrong arn, got {description:?}"
    );
    assert_eq!(
        description
            .get("computePlatform")
            .and_then(serde_json::Value::as_str),
        Some(case.expected_compute_platform()),
        "case {case:?} ({label}): wrong computePlatform, got {description:?}"
    );
    assert_eq!(
        description
            .pointer("/agentOrchestrationConfig/profilingEnabled")
            .and_then(serde_json::Value::as_bool),
        Some(case.expected_profiling_enabled()),
        "case {case:?} ({label}): wrong profilingEnabled, got {description:?}"
    );
    let tags = description
        .get("tags")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?} ({label}): no tags in {description:?}"));
    assert_eq!(
        tags.len(),
        case.tags.len(),
        "case {case:?} ({label}): wrong tag count, got {tags:?}"
    );
    for (key, value) in &case.tags {
        assert_eq!(
            tags.get(*key).and_then(serde_json::Value::as_str),
            Some(*value),
            "case {case:?} ({label}): wrong tag {key}, got {tags:?}"
        );
    }
    assert!(
        description.get("createdAt").is_some(),
        "case {case:?} ({label}): no createdAt in {description:?}"
    );
    assert!(
        description.get("updatedAt").is_some(),
        "case {case:?} ({label}): no updatedAt in {description:?}"
    );
    // No agent has pinged back and no profile has been posted for any group these cases
    // build, so there is nothing to report in `profilingStatus`: a read carries the
    // container with none of its three instants in it, and the create's payload leaves the
    // member out altogether.
    let status = description.get("profilingStatus");
    if reports_profiling_status {
        assert_eq!(
            status.and_then(serde_json::Value::as_object).map(|o| o.len()),
            Some(0),
            "case {case:?} ({label}): a read must report an empty profilingStatus, got \
             {description:?}"
        );
    } else {
        assert_eq!(
            status, None,
            "case {case:?} ({label}): a create has no profiling activity to report, so \
             profilingStatus must be absent, got {description:?}"
        );
    }
}

#[tokio::test]
async fn a_created_group_is_described_from_what_was_stored() {
    let harness = Harness::new();

    for (index, case) in group_cases().iter().enumerate() {
        let created = harness.create(case, &format!("tok-{index}")).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateProfilingGroup must answer 201, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_description = created.json();
        assert_description(case, "create", false, &created_description);

        let described = harness.describe(&case.name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeProfilingGroup must answer 200, got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );
        let described_description = described.json();
        assert_description(case, "describe", true, &described_description);
        // The decisive assertion: describe answers with the record create stored, down to
        // the create-time timestamps, not with a freshly built reply. The two bodies differ
        // in exactly one member — the `profilingStatus` container a read carries and a
        // create omits — so the comparison is of everything else, member for member.
        let mut described_without_status = described_description.clone();
        assert!(
            described_without_status
                .as_object_mut()
                .expect("a JSON object")
                .remove("profilingStatus")
                .is_some(),
            "case {case:?}: a read must carry profilingStatus, got {described_description:?}"
        );
        assert_eq!(
            described_without_status, created_description,
            "case {case:?}: describe must return the stored description verbatim"
        );
        assert_eq!(
            described_description.pointer("/profilingStatus/latestAgentOrchestratedAt"),
            None,
            "case {case:?}: no agent has pinged back yet, got {described_description:?}"
        );
        assert_eq!(
            described_description.pointer("/profilingStatus/latestAgentProfileReportedAt"),
            None,
            "case {case:?}: no profile has been posted yet, got {described_description:?}"
        );
    }

    let missing = harness.describe("no-such-group").await;
    assert_eq!(
        missing.status, 404,
        "an unknown group must answer 404, got {} {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "an unknown group must answer ResourceNotFoundException, got {}",
        String::from_utf8_lossy(&missing.body)
    );
}

#[tokio::test]
async fn a_repeated_create_is_idempotent_on_its_client_token_and_conflicts_without_it() {
    let harness = Harness::new();

    for (index, case) in group_cases().iter().enumerate() {
        let token = format!("tok-{index}");
        let first = harness.create(case, &token).await;
        assert_eq!(first.status, 201, "case {case:?}: first create must be 201");

        let replay = harness.create(case, &token).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: replaying a clientToken must succeed, got {} {}",
            replay.status,
            String::from_utf8_lossy(&replay.body)
        );
        assert_eq!(
            replay.json(),
            first.json(),
            "case {case:?}: a clientToken replay must return the group it created"
        );

        let conflict = harness.create(case, &format!("other-{index}")).await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: a new clientToken on an existing name must be 409, got {} {}",
            conflict.status,
            String::from_utf8_lossy(&conflict.body)
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            String::from_utf8_lossy(&conflict.body)
        );
    }
}

#[tokio::test]
async fn configure_agent_answers_from_the_stored_orchestration_config_and_records_the_ping() {
    let harness = Harness::new();

    for (index, case) in group_cases().iter().enumerate() {
        assert_eq!(
            harness.create(case, &format!("tok-{index}")).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        let configured = harness.configure_agent(&case.name).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: ConfigureAgent must answer 200, got {} {}",
            configured.status,
            String::from_utf8_lossy(&configured.body)
        );
        let configuration = configured.json();
        assert_eq!(
            configuration
                .get("shouldProfile")
                .and_then(serde_json::Value::as_bool),
            Some(case.expected_profiling_enabled()),
            "case {case:?}: shouldProfile must come from the stored \
             agentOrchestrationConfig, got {configuration:?}"
        );
        let period = configuration
            .get("periodInSeconds")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no periodInSeconds in {configuration:?}"));
        assert!(
            period > 0,
            "case {case:?}: periodInSeconds must be positive, got {period}"
        );
        // The agent-parameter keys are the whole `AgentParameterField` catalogue the
        // model declares (smithy_query query="shape", shape="AgentParameterField").
        let parameters = configuration
            .get("agentParameters")
            .and_then(serde_json::Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no agentParameters in {configuration:?}"));
        for field in [
            "SamplingIntervalInMilliseconds",
            "ReportingIntervalInMilliseconds",
            "MinimumTimeForReportingInMilliseconds",
            "MemoryUsageLimitPercent",
            "MaxStackDepth",
        ] {
            assert!(
                parameters.contains_key(field),
                "case {case:?}: agentParameters is missing {field}, got {parameters:?}"
            );
        }
        assert_eq!(
            parameters
                .get("ReportingIntervalInMilliseconds")
                .and_then(serde_json::Value::as_str),
            Some((period * 1_000).to_string().as_str()),
            "case {case:?}: the reporting interval must agree with periodInSeconds \
             {period}, got {parameters:?}"
        );

        let described = harness.describe(&case.name).await;
        let pinged_at = described
            .json()
            .pointer("/profilingStatus/latestAgentOrchestratedAt")
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: ConfigureAgent must record latestAgentOrchestratedAt, got {}",
                    String::from_utf8_lossy(&described.body)
                )
            });
        // Describing again reports the SAME recorded ping: the timestamp is stored,
        // not minted per response.
        assert_eq!(
            harness
                .describe(&case.name)
                .await
                .json()
                .pointer("/profilingStatus/latestAgentOrchestratedAt")
                .cloned(),
            Some(pinged_at),
            "case {case:?}: latestAgentOrchestratedAt must be stable across reads"
        );
    }

    let missing = harness.configure_agent("no-such-group").await;
    assert_eq!(
        missing.status, 404,
        "ConfigureAgent on an unknown group must answer 404, got {} {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
}

/// One posted agent profile. Rows differ in payload size, byte content and media
/// type — the two media types are the ones the model's `GetProfile$contentType` doc
/// names: "either application/json or the default application/x-amzn-ion".
struct ProfileCase {
    label: &'static str,
    content_type: &'static str,
    bytes: Vec<u8>,
}

fn profile_cases() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            label: "one byte",
            content_type: "application/json",
            bytes: vec![b'{'],
        },
        ProfileCase {
            label: "small json",
            content_type: "application/json",
            bytes: br#"{"start":1,"frames":["A","B"]}"#.to_vec(),
        },
        ProfileCase {
            label: "ion with every byte value",
            content_type: "application/x-amzn-ion",
            bytes: (0..=255u8).collect(),
        },
        ProfileCase {
            label: "64 KiB of ion",
            content_type: "application/x-amzn-ion",
            bytes: (0..65_536u32).map(|i| (i % 251) as u8).collect(),
        },
    ]
}

#[tokio::test]
async fn a_posted_agent_profile_comes_back_byte_identical() {
    let harness = Harness::new();

    for (index, profile) in profile_cases().iter().enumerate() {
        let case = GroupCase {
            name: format!("profiled-{index}"),
            compute_platform: None,
            profiling_enabled: Some(true),
            tags: BTreeMap::new(),
        };
        assert_eq!(
            harness.create(&case, &format!("tok-{index}")).await.status,
            201,
            "{}: create must succeed",
            profile.label
        );

        // Nothing posted yet: there is no profile to get.
        let empty = harness.get_profile(&case.name, "").await;
        assert_eq!(
            empty.status, 404,
            "{}: GetProfile before any post must answer 404, got {} {}",
            profile.label,
            empty.status,
            String::from_utf8_lossy(&empty.body)
        );

        let posted = harness
            .post_agent_profile(
                &case.name,
                profile.content_type,
                Some(&format!("ptok-{index}")),
                profile.bytes.clone(),
            )
            .await;
        assert_eq!(
            posted.status, 204,
            "{}: PostAgentProfile must answer 204, got {} {}",
            profile.label,
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );

        let fetched = harness.get_profile(&case.name, "").await;
        assert_eq!(
            fetched.status, 200,
            "{}: GetProfile must answer 200, got {} {}",
            profile.label,
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.body, profile.bytes,
            "{}: GetProfile must return the posted bytes unchanged (got {} bytes, posted {})",
            profile.label,
            fetched.body.len(),
            profile.bytes.len()
        );
        assert_eq!(
            fetched.content_type.as_deref(),
            Some(profile.content_type),
            "{}: GetProfile must report the content type the profile was posted under",
            profile.label
        );

        let described = harness.describe(&case.name).await;
        assert!(
            described
                .json()
                .pointer("/profilingStatus/latestAgentProfileReportedAt")
                .is_some(),
            "{}: PostAgentProfile must record latestAgentProfileReportedAt, got {}",
            profile.label,
            String::from_utf8_lossy(&described.body)
        );

        // Re-posting under the same `profileToken` is the SAME post: the second body
        // must not replace the stored profile.
        assert_eq!(
            harness
                .post_agent_profile(
                    &case.name,
                    "application/json",
                    Some(&format!("ptok-{index}")),
                    b"{\"replaced\":true}".to_vec(),
                )
                .await
                .status,
            204,
            "{}: replaying a profileToken must still answer 204",
            profile.label
        );
        let after_replay = harness.get_profile(&case.name, "").await;
        assert_eq!(
            after_replay.body, profile.bytes,
            "{}: a profileToken replay must not store a second profile",
            profile.label
        );
    }

    let missing = harness
        .post_agent_profile("no-such-group", "application/json", None, b"{}".to_vec())
        .await;
    assert_eq!(
        missing.status, 404,
        "PostAgentProfile on an unknown group must answer 404, got {} {}",
        missing.status,
        String::from_utf8_lossy(&missing.body)
    );
    assert_eq!(
        harness.get_profile("no-such-group", "").await.status,
        404,
        "GetProfile on an unknown group must answer 404"
    );
}

#[tokio::test]
async fn listing_pages_over_every_stored_group_once() {
    let harness = Harness::new();

    let cases = group_cases();
    let mut expected: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(
            harness.create(case, &format!("tok-{index}")).await.status,
            201,
            "case {case:?}: create must succeed"
        );
        expected.push(case.name.clone());
    }
    expected.sort();

    // Rows: every page size from 1 to one past the group count, plus the modelled
    // `maxResults` upper bound (1..1000) and no limit at all.
    let page_sizes: Vec<Option<usize>> = (1..=expected.len() + 1)
        .map(Some)
        .chain([Some(1000), None])
        .collect();

    for page_size in page_sizes {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = next.as_ref() {
                query.push(format!("nextToken={token}"));
            }
            let uri = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let answer = harness.list(&uri).await;
            assert_eq!(
                answer.status, 200,
                "page size {page_size:?}: ListProfilingGroups must answer 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let body = answer.json();
            let names = body
                .get("profilingGroupNames")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| {
                    panic!("page size {page_size:?}: no profilingGroupNames in {body:?}")
                });
            if let Some(size) = page_size {
                assert!(
                    names.len() <= size,
                    "page size {page_size:?}: a page returned {} names",
                    names.len()
                );
            }
            assert_eq!(
                body.get("profilingGroups"),
                None,
                "page size {page_size:?}: profilingGroups must be absent unless \
                 includeDescription is true, got {body:?}"
            );
            seen.extend(
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 3,
                "page size {page_size:?}: paging did not terminate after {pages} pages"
            );
            next = body
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size:?}: paging must visit every stored group exactly once"
        );
    }

    // `includeDescription=true` adds the same descriptions Describe returns.
    let answer = harness.list("?includeDescription=true").await;
    assert_eq!(answer.status, 200, "ListProfilingGroups must answer 200");
    let body = answer.json();
    let descriptions = body
        .get("profilingGroups")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("includeDescription=true returned no profilingGroups: {body:?}"));
    assert_eq!(
        descriptions.len(),
        expected.len(),
        "includeDescription=true must describe every group, got {descriptions:?}"
    );
    for (name, description) in expected.iter().zip(descriptions) {
        let case = cases
            .iter()
            .find(|case| &case.name == name)
            .expect("a created case");
        assert_description(case, "list", true, description);
        assert_eq!(
            description,
            &harness.describe(name).await.json(),
            "case {case:?}: list and describe must report the same stored description"
        );
    }

    // A token this service never issued is refused, not served as page zero or as an
    // empty page: neither an unparseable one nor a numeric one past the end.
    for token in ["not-a-token", "9999"] {
        let refused = harness.list(&format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "nextToken {token:?} must be refused, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }
}

#[tokio::test]
async fn deleting_a_group_removes_it_and_its_profiles() {
    let harness = Harness::new();

    for (index, case) in group_cases().iter().enumerate() {
        let token = format!("tok-{index}");
        assert_eq!(
            harness.create(case, &token).await.status,
            201,
            "case {case:?}: create must succeed"
        );
        assert_eq!(
            harness
                .post_agent_profile(&case.name, "application/json", None, b"{\"a\":1}".to_vec())
                .await
                .status,
            204,
            "case {case:?}: post must succeed"
        );

        let deleted = harness.delete(&case.name).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeleteProfilingGroup must answer 204, got {} {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );

        assert_eq!(
            harness.describe(&case.name).await.status,
            404,
            "case {case:?}: describing a deleted group must answer 404"
        );
        assert_eq!(
            harness.get_profile(&case.name, "").await.status,
            404,
            "case {case:?}: getting a deleted group's profile must answer 404"
        );
        assert_eq!(
            harness.delete(&case.name).await.status,
            404,
            "case {case:?}: deleting it again must answer 404"
        );
        let listed = harness.list("").await;
        assert!(
            !listed
                .json()
                .get("profilingGroupNames")
                .and_then(serde_json::Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .any(|name| name.as_str() == Some(case.name.as_str())),
            "case {case:?}: a deleted group must disappear from ListProfilingGroups, got {}",
            String::from_utf8_lossy(&listed.body)
        );

        // The same clientToken creates a FRESH group after the delete: the token must
        // not replay the group that was removed.
        let recreated = harness.create(case, &token).await;
        assert_eq!(
            recreated.status, 201,
            "case {case:?}: re-creating after a delete must answer 201, got {} {}",
            recreated.status,
            String::from_utf8_lossy(&recreated.body)
        );
        assert_eq!(
            harness.get_profile(&case.name, "").await.status,
            404,
            "case {case:?}: the re-created group must carry none of the deleted profiles"
        );
    }
}

#[tokio::test]
async fn posting_profiles_stays_inside_the_per_group_retention_bound() {
    use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
        MAX_RETAINED_PROFILES_PER_GROUP, MAX_RETAINED_PROFILE_BYTES,
        MAX_RETAINED_PROFILE_BYTES_PER_GROUP, MAX_RETAINED_PROFILE_TOKENS_PER_GROUP,
    };

    let harness = Harness::new();

    // Rows: payload sizes that reach the bound by count (many tiny profiles) and by
    // total bytes (few large ones). Either way the group must stay inside BOTH caps
    // and must still serve the profile posted last.
    for (label, payload_bytes, posts) in [
        ("many tiny profiles", 8usize, MAX_RETAINED_PROFILES_PER_GROUP * 3 + 1),
        (
            "few large profiles",
            MAX_RETAINED_PROFILE_BYTES_PER_GROUP / 4 + 1,
            6,
        ),
    ] {
        let case = GroupCase {
            name: format!("retention-{}", payload_bytes),
            compute_platform: None,
            profiling_enabled: Some(true),
            tags: BTreeMap::new(),
        };
        assert_eq!(
            harness.create(&case, "tok-retention").await.status,
            201,
            "{label}: create must succeed"
        );

        let mut last = Vec::new();
        for post in 0..posts {
            last = vec![u8::try_from(post % 256).unwrap_or(0); payload_bytes];
            assert_eq!(
                harness
                    .post_agent_profile(
                        &case.name,
                        "application/json",
                        Some(&format!("ptok-{post}")),
                        last.clone(),
                    )
                    .await
                    .status,
                204,
                "{label}: post {post} must answer 204"
            );
            let (profiles, bytes, tokens) = harness.retained(&case.name);
            assert!(
                profiles <= MAX_RETAINED_PROFILES_PER_GROUP,
                "{label}: after {} posts the group retains {profiles} profiles, \
                 above the {MAX_RETAINED_PROFILES_PER_GROUP} bound",
                post + 1
            );
            assert!(
                bytes <= MAX_RETAINED_PROFILE_BYTES_PER_GROUP || profiles == 1,
                "{label}: after {} posts the group retains {bytes} bytes, above the \
                 {MAX_RETAINED_PROFILE_BYTES_PER_GROUP} bound",
                post + 1
            );
            assert!(
                tokens <= MAX_RETAINED_PROFILE_TOKENS_PER_GROUP,
                "{label}: after {} posts the group remembers {tokens} tokens, above \
                 the {MAX_RETAINED_PROFILE_TOKENS_PER_GROUP} bound",
                post + 1
            );
            let total = harness.retained_profile_bytes();
            assert!(
                total <= MAX_RETAINED_PROFILE_BYTES,
                "{label}: after {} posts the store retains {total} profile bytes, \
                 above the store-wide {MAX_RETAINED_PROFILE_BYTES} budget",
                post + 1
            );
        }

        let fetched = harness.get_profile(&case.name, "").await;
        assert_eq!(
            fetched.status, 200,
            "{label}: the newest profile must still be readable, got {} {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.body, last,
            "{label}: GetProfile must return the profile posted last ({} bytes)",
            last.len()
        );
    }
}

#[tokio::test]
async fn creating_past_the_profiling_group_quota_is_refused_and_recovers_after_a_delete() {
    use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::MAX_PROFILING_GROUPS;

    let harness = Harness::new();
    let case_for = |index: usize| GroupCase {
        name: format!("quota-{index}"),
        compute_platform: None,
        profiling_enabled: Some(true),
        tags: BTreeMap::new(),
    };

    for index in 0..MAX_PROFILING_GROUPS {
        let answer = harness.create(&case_for(index), &format!("tok-{index}")).await;
        assert_eq!(
            answer.status, 201,
            "group {index} of the quota must be created, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let over = harness
        .create(&case_for(MAX_PROFILING_GROUPS), "tok-over")
        .await;
    assert_eq!(
        over.status, 402,
        "a create past the quota of {MAX_PROFILING_GROUPS} must answer 402, got {} {}",
        over.status,
        String::from_utf8_lossy(&over.body)
    );
    assert_eq!(
        over.error_code(),
        "ServiceQuotaExceededException",
        "a create past the quota must answer ServiceQuotaExceededException, got {}",
        String::from_utf8_lossy(&over.body)
    );

    // Freeing a group frees the quota it held.
    assert_eq!(
        harness.delete(&case_for(0).name).await.status,
        204,
        "deleting a group must succeed"
    );
    let retry = harness
        .create(&case_for(MAX_PROFILING_GROUPS), "tok-over")
        .await;
    assert_eq!(
        retry.status, 201,
        "a create after a delete must succeed, got {} {}",
        retry.status,
        String::from_utf8_lossy(&retry.body)
    );
}

#[tokio::test]
async fn get_profile_selects_by_the_requested_time_range() {
    let harness = Harness::new();
    let case = GroupCase {
        name: "windowed".to_string(),
        compute_platform: None,
        profiling_enabled: Some(true),
        tags: BTreeMap::new(),
    };
    assert_eq!(harness.create(&case, "tok-window").await.status, 201);
    let body = b"{\"frames\":[]}".to_vec();
    assert_eq!(
        harness
            .post_agent_profile(&case.name, "application/json", None, body.clone())
            .await
            .status,
        204
    );

    let reported_at = harness
        .describe(&case.name)
        .await
        .json()
        .pointer("/profilingStatus/latestAgentProfileReportedAt")
        .cloned()
        .expect("the post must be recorded");

    // Rows: each way the operation's doc allows a window to be named. A window that
    // contains the receipt time serves the profile; one that does not is a 404.
    let cases: Vec<(&str, &str, u16)> = vec![
        ("no parameters", "", 200),
        ("period alone (the latest profile)", "?period=PT1H", 200),
        (
            "a start before the profile",
            "?startTime=2000-01-01T00:00:00Z",
            200,
        ),
        (
            "a start after the profile",
            "?startTime=2999-01-01T00:00:00Z",
            404,
        ),
        (
            "an end after the profile",
            "?endTime=2999-01-01T00:00:00Z",
            200,
        ),
        (
            "an end before the profile",
            "?endTime=2000-01-01T00:00:00Z",
            404,
        ),
        (
            "a start and an end around the profile",
            "?startTime=2000-01-01T00:00:00Z&endTime=2999-01-01T00:00:00Z",
            200,
        ),
        (
            "a day-long window starting long before the profile",
            "?startTime=2000-01-01T00:00:00Z&period=P1D",
            404,
        ),
        (
            "a day-long window ending far after the profile",
            "?endTime=2999-01-01T00:00:00Z&period=P1D",
            404,
        ),
        // The operation's doc: "If you specify all 3 parameters, an exception is thrown."
        (
            "all three parameters",
            "?startTime=2000-01-01T00:00:00Z&endTime=2999-01-01T00:00:00Z&period=P1D",
            400,
        ),
        // Only the three aggregation periods the doc offers are selectable.
        ("an unoffered period", "?period=PT7M", 400),
        // Truncating the stored profile's call stacks is not something this emulator
        // can do, so it is refused rather than answered with an untruncated profile.
        ("maxDepth", "?maxDepth=2", 400),
    ];

    for (label, query, expected_status) in cases {
        let answer = harness.get_profile(&case.name, query).await;
        assert_eq!(
            answer.status, expected_status,
            "case {label:?} ({query}): expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if answer.status == 200 {
            assert_eq!(
                answer.body, body,
                "case {label:?} ({query}): must return the posted bytes"
            );
        }
    }

    // `accept` asking for a media type the stored profile is not in cannot be served
    // without transcoding the profile, so it is refused; one that accepts the stored
    // media type is served.
    for (label, accept, expected_status) in [
        ("the stored media type", "application/json", 200u16),
        ("any media type", "*/*", 200),
        ("a media type the profile is not in", "application/x-amzn-ion", 400),
    ] {
        let answer = harness
            .get_profile_accepting(&case.name, "", Some(accept))
            .await;
        assert_eq!(
            answer.status, expected_status,
            "case {label:?} (accept: {accept}): expected HTTP {expected_status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Reading never changed what the next caller sees.
    assert_eq!(
        harness
            .describe(&case.name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .cloned(),
        Some(reported_at),
        "GetProfile must not change the recorded profile-report time"
    );
}
