// SPDX-License-Identifier: CC0-1.0

//! Which ERROR SHAPE this service answers a handler-enforced INPUT refusal with.
//!
//! The rule every row asserts: when a handler refuses bad client input for an operation
//! whose own model lists `ValidationException` among its errors
//! (`smithy_query(query="operation", operation="CreateProfilingGroup,GetProfile")` ->
//! `{"shape": "ValidationException", "errorKind": "client", "httpError": 400}`), the error
//! value it returns IS that modelled shape — `com.amazonaws.codeguruprofiler#ValidationException`
//! — and never a shape from outside this service's model that merely renders the same
//! status and code. A shape in the model is the only one that can carry a member the model
//! grows beside `message`, and it is the one a generated client deserializes.
//!
//! Each row is asserted on the value the HANDLER returns, called directly with a typed
//! input: the wire form of a restJson1 error carries only the status, the `__type` code and
//! `message`, so the shape identity — the thing the rule is about — is observable only on
//! the typed error. The identity is read off the schema the error channel renders with
//! (`OperationError::variant_schema`). Every row is then driven over HTTP too, so the
//! status and code a signed SDK request observes are pinned beside it.
//!
//! The rows differ along what the rule generalises over: the OPERATION (a create, a read),
//! and the refusal class inside each — a value outside a closed catalogue, a map past the
//! published tagging limits (count, key length, value length), a window member without the
//! companion its own doc requires, a window named three ways at once, a length that names
//! no duration, a pair of bounds that is no interval, and a range longer than the
//! operation's documented maximum, named by a pair of bounds and named by a length alone.
//!
//! The boundary of the rule is asserted too: input these operations ACCEPT is not refused,
//! and what a create stores reads back through `DescribeProfilingGroup`.
//!
//! The WORDING of each of these refusals is pinned by the suites that own it
//! ([`crate::refusal_message_rules`], [`crate::get_profile_window_companion`],
//! [`crate::get_profile_interval`], [`crate::get_profile_window_length`]); what is new here
//! is the shape that carries it. The emulator and the HTTP plumbing are the shared
//! [`crate::profiling_group_lifecycle`] harness, not a fork of it.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_types::DateTime;
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_codeguruprofiler_codeguruprofiler as codeguruprofiler;

use codeguruprofiler::ops::create_profiling_group::{
    CreateProfilingGroup, CreateProfilingGroupError, CreateProfilingGroupErrorModeled,
};
use codeguruprofiler::ops::get_profile::{GetProfile, GetProfileError, GetProfileErrorModeled};
use codeguruprofiler::ops::types::{
    COMPUTE_PLATFORM_VALUES, CreateProfilingGroupInput, CreateProfilingGroupOutput, GetProfileInput,
    GetProfileOutput,
};
use codeguruprofiler::scaffolded::Scaffolded;
use codeguruprofiler::state;

use crate::profiling_group_lifecycle::Harness;

/// The shape id this service's model declares for a client-input refusal, which such a
/// refusal has to carry — `smithy_query(query="shape", shape="ValidationException")`.
const MODELLED_VALIDATION_SHAPE: &str = "com.amazonaws.codeguruprofiler#ValidationException";

/// The wire error code that shape renders with on this service's restJson1 protocol.
const VALIDATION_CODE: &str = "ValidationException";

/// The group the `GetProfile` rows are asked about, created through the API, so a row that
/// is NOT refused reaches the window the operation reads over rather than a missing group.
const GROUP: &str = "cgp-refusal-shape";

/// A current-era instant the window rows are built around (2026-01-01T00:00:00Z), in epoch
/// seconds.
const START_SECS: i64 = 1_767_225_600;

/// One day in seconds, for building the windows below.
const DAY_SECS: i64 = 24 * 60 * 60;

/// What the `GetProfile` doc calls the range it bounds, and the maximum it allows: "The
/// maximum time range allowed is 7 days" (`smithy_query(query="doc",
/// shape="GetProfileRequest")`). Written out here so this suite pins the wire text rather
/// than reading the constant the handler states it with.
const TIME_RANGE_SUBJECT: &str = "the time range for the requested aggregated profile";
const MAX_TIME_RANGE_TEXT: &str = "P7D";

/// An emulator that reaches ONE store two ways: through the handler, with a typed input, so
/// a refusal's own type is observable, and over HTTP through the shared harness.
struct Refusals {
    harness: Harness,
    /// Mounted beside the harness's own copy on the SAME engine, for the service identity
    /// an [`OperationContext`] carries.
    service: codeguruprofiler::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Refusals {
    fn new() -> Self {
        let harness = Harness::new();
        let service = codeguruprofiler::EmulatedService::new(Arc::clone(harness.engine()));
        let router = harness.router();
        Self {
            harness,
            service,
            router,
            parts: request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// The create as the HANDLER answers it.
    async fn create(
        &self,
        input: CreateProfilingGroupInput,
    ) -> Result<CreateProfilingGroupOutput, CreateProfilingGroupError> {
        CreateProfilingGroup::new(Arc::clone(self.harness.engine()))
            .handle(input, &self.ctx())
            .await
    }

    /// The read as the HANDLER answers it.
    async fn get_profile(
        &self,
        input: GetProfileInput,
    ) -> Result<GetProfileOutput, GetProfileError> {
        GetProfile::new(Arc::clone(self.harness.engine()))
            .handle(input, &self.ctx())
            .await
    }
}

/// Request metadata naming this service's signing name and a Region, which is how a request
/// names the Region a handler reads.
fn request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/profilingGroups")
        .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// The rule, asserted on the typed refusal: the shape the model declares, its status and
/// the code it renders with.
fn assert_modelled_validation<E: OperationError>(case: &str, error: &E) {
    let shape = error.variant_schema().shape_id().to_string();
    assert_eq!(
        shape, MODELLED_VALIDATION_SHAPE,
        "case {case:?}: expected the modelled shape {MODELLED_VALIDATION_SHAPE}, got {shape:?}"
    );
    assert_eq!(
        error.http_status(),
        400,
        "case {case:?}: expected 400, got {}",
        error.http_status()
    );
    assert_eq!(
        error.shape_name(),
        VALIDATION_CODE,
        "case {case:?}: expected the wire code {VALIDATION_CODE}, got {:?}",
        error.shape_name()
    );
}

/// The `message` of a create refusal that MUST be this operation's own modelled
/// `ValidationException`, or a panic naming the case.
fn create_refusal_message(case: &str, error: &CreateProfilingGroupError) -> String {
    match error {
        Scaffolded::Modeled(CreateProfilingGroupErrorModeled::ValidationException(refusal)) => {
            refusal.message.clone()
        }
        other => panic!(
            "case {case:?}: the refusal must be the ValidationException this operation's model \
             declares, got {other:?}"
        ),
    }
}

/// The `message` of a read refusal that MUST be this operation's own modelled
/// `ValidationException`, or a panic naming the case.
fn get_profile_refusal_message(case: &str, error: &GetProfileError) -> String {
    match error {
        Scaffolded::Modeled(GetProfileErrorModeled::ValidationException(refusal)) => {
            refusal.message.clone()
        }
        other => panic!(
            "case {case:?}: the refusal must be the ValidationException this operation's model \
             declares, got {other:?}"
        ),
    }
}

/// A tag map from borrowed pairs.
fn tags(pairs: &[(String, String)]) -> HashMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One `CreateProfilingGroup` request this operation refuses on its input.
struct CreateRow {
    case: &'static str,
    name: &'static str,
    compute_platform: Option<String>,
    tags: HashMap<String, String>,
    /// The sentence the handler states the refusal with.
    expected: String,
}

impl CreateRow {
    fn input(&self, client_token: &str) -> CreateProfilingGroupInput {
        CreateProfilingGroupInput {
            profiling_group_name: self.name.to_string(),
            compute_platform: self.compute_platform.clone(),
            client_token: client_token.to_string(),
            agent_orchestration_config: None,
            tags: Some(self.tags.clone()),
        }
    }

    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert(
            "profilingGroupName".to_string(),
            serde_json::Value::String(self.name.to_string()),
        );
        if let Some(platform) = self.compute_platform.as_ref() {
            body.insert(
                "computePlatform".to_string(),
                serde_json::Value::String(platform.clone()),
            );
        }
        body.insert(
            "tags".to_string(),
            serde_json::Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
                    .collect(),
            ),
        );
        serde_json::Value::Object(body)
    }
}

/// What a value outside the `ComputePlatform` catalogue is refused with. The catalogue is
/// the service's own enum value set (`ops::types::COMPUTE_PLATFORM_VALUES`, from
/// `smithy_query(query="shape", shape="ComputePlatform")`), so the sentence names whatever
/// the model declares rather than a list written out here.
fn platform_refusal(value: &str) -> String {
    format!(
        "computePlatform must be one of {}, got {value}",
        COMPUTE_PLATFORM_VALUES.join(", ")
    )
}

/// The create rows: a value outside a closed catalogue, and each of the three published
/// tagging limits (`MAX_TAGS_PER_RESOURCE`, `MAX_TAG_KEY_CHARS`, `MAX_TAG_VALUE_CHARS` —
/// AWS's tagging quotas, which `state` states once for every tagging path).
fn create_rows() -> Vec<CreateRow> {
    let over_long_key = "k".repeat(state::MAX_TAG_KEY_CHARS + 1);
    let over_long_value = "v".repeat(state::MAX_TAG_VALUE_CHARS + 1);
    let too_many: Vec<(String, String)> = (0..=state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), "value".to_string()))
        .collect();
    vec![
        CreateRow {
            case: "a compute platform outside the catalogue",
            name: "cgp-refusal-platform",
            compute_platform: Some("NotAPlatform".to_string()),
            tags: HashMap::new(),
            expected: platform_refusal("NotAPlatform"),
        },
        CreateRow {
            case: "a catalogue entry in the wrong case",
            name: "cgp-refusal-platform-case",
            compute_platform: Some("default".to_string()),
            tags: HashMap::new(),
            expected: platform_refusal("default"),
        },
        CreateRow {
            case: "an empty compute platform",
            name: "cgp-refusal-platform-empty",
            compute_platform: Some(String::new()),
            tags: HashMap::new(),
            expected: platform_refusal(""),
        },
        CreateRow {
            case: "one tag past the per-resource maximum",
            name: "cgp-refusal-tag-count",
            compute_platform: None,
            tags: tags(&too_many),
            expected: format!(
                "Value at 'tags' failed to satisfy constraint: Member must have length less \
                 than or equal to {}",
                state::MAX_TAGS_PER_RESOURCE
            ),
        },
        CreateRow {
            case: "a tag key one character past the maximum",
            name: "cgp-refusal-tag-key",
            compute_platform: None,
            tags: tags(&[(over_long_key.clone(), "value".to_string())]),
            expected: format!(
                "Value '{over_long_key}' at 'tags' failed to satisfy constraint: Map keys must \
                 have length between 1 and {}",
                state::MAX_TAG_KEY_CHARS
            ),
        },
        CreateRow {
            case: "an empty tag key",
            name: "cgp-refusal-tag-key-empty",
            compute_platform: None,
            tags: tags(&[(String::new(), "value".to_string())]),
            expected: format!(
                "Value '' at 'tags' failed to satisfy constraint: Map keys must have length \
                 between 1 and {}",
                state::MAX_TAG_KEY_CHARS
            ),
        },
        CreateRow {
            case: "a tag value one character past the maximum",
            name: "cgp-refusal-tag-value",
            compute_platform: None,
            tags: tags(&[("retention".to_string(), over_long_value)]),
            expected: format!(
                "Value at 'tags.retention' failed to satisfy constraint: Member must have \
                 length less than or equal to {}",
                state::MAX_TAG_VALUE_CHARS
            ),
        },
    ]
}

/// One `GetProfile` request this operation refuses on its window members.
struct GetRow {
    case: &'static str,
    start_time: Option<DateTime>,
    end_time: Option<DateTime>,
    period: Option<&'static str>,
    /// The sentence the handler states the refusal with.
    expected: String,
}

impl GetRow {
    fn input(&self) -> GetProfileInput {
        GetProfileInput {
            profiling_group_name: GROUP.to_string(),
            start_time: self.start_time,
            end_time: self.end_time,
            period: self.period.map(str::to_string),
            max_depth: None,
            accept: None,
        }
    }

    /// The same window as a signed request names it: each member it carried, spelled the
    /// way this service spells an instant.
    fn query(&self) -> String {
        let mut members: Vec<String> = Vec::new();
        if let Some(start) = self.start_time {
            members.push(format!("startTime={}", state::iso8601(start)));
        }
        if let Some(end) = self.end_time {
            members.push(format!("endTime={}", state::iso8601(end)));
        }
        if let Some(period) = self.period {
            members.push(format!("period={period}"));
        }
        if members.is_empty() {
            String::new()
        } else {
            format!("?{}", members.join("&"))
        }
    }
}

/// The window rows: the companion rule on each bound, the trio named three ways at once, a
/// length that is no duration, a pair that is no interval, and a range past the documented
/// maximum named both ways a request can name one.
fn get_rows() -> Vec<GetRow> {
    let start = DateTime::from_secs(START_SECS);
    let hour_later = DateTime::from_secs(START_SECS + 3_600);
    let eight_days_later = DateTime::from_secs(START_SECS + 8 * DAY_SECS);
    vec![
        GetRow {
            case: "a start without its companion",
            start_time: Some(start),
            end_time: None,
            period: None,
            expected:
                "If you specify startTime, then you must also specify period or endTime, but \
                 not both"
                    .to_string(),
        },
        GetRow {
            case: "an end without its companion",
            start_time: None,
            end_time: Some(hour_later),
            period: None,
            expected:
                "If you specify endTime, then you must also specify period or startTime, but \
                 not both"
                    .to_string(),
        },
        GetRow {
            case: "all three window members at once",
            start_time: Some(start),
            end_time: Some(hour_later),
            period: Some("PT1H"),
            expected: state::OVER_DETERMINED_WINDOW_MESSAGE.to_string(),
        },
        GetRow {
            case: "a length that names no duration",
            start_time: None,
            end_time: None,
            period: Some("yesterday"),
            expected: state::period_validation_message("yesterday"),
        },
        GetRow {
            case: "a pair of bounds in the wrong order",
            start_time: Some(hour_later),
            end_time: Some(start),
            period: None,
            expected: format!(
                "Interval start time ({}) must be before end time ({})",
                state::iso8601(hour_later),
                state::iso8601(start)
            ),
        },
        GetRow {
            case: "a zero-length window",
            start_time: Some(start),
            end_time: Some(start),
            period: None,
            expected: format!(
                "Interval start time ({}) must be before end time ({})",
                state::iso8601(start),
                state::iso8601(start)
            ),
        },
        GetRow {
            case: "a pair of bounds longer than the documented maximum",
            start_time: Some(start),
            end_time: Some(eight_days_later),
            period: None,
            expected: format!(
                "{TIME_RANGE_SUBJECT} must be at most {MAX_TIME_RANGE_TEXT} long, got {} to {}",
                state::iso8601(start),
                state::iso8601(eight_days_later)
            ),
        },
        GetRow {
            case: "a length alone longer than the documented maximum",
            start_time: None,
            end_time: None,
            period: Some("P8D"),
            expected: format!(
                "{TIME_RANGE_SUBJECT} must be at most {MAX_TIME_RANGE_TEXT} long, got P8D"
            ),
        },
    ]
}

/// The group every `GetProfile` row is asked about, created over HTTP like any other.
async fn create_group(refusals: &Refusals) {
    let created = refusals
        .harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken={GROUP}-token"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": GROUP }))
                .expect("a serializable body"),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "creating {GROUP} must succeed, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

#[tokio::test]
async fn a_create_refuses_bad_input_with_the_shape_its_model_declares() {
    let refusals = Refusals::new();

    for (index, row) in create_rows().into_iter().enumerate() {
        let case = row.case;
        let refused = refusals
            .create(row.input(&format!("token-{index}")))
            .await
            .expect_err(&format!("case {case:?}: this input must be refused"));

        assert_modelled_validation(case, &refused);
        let message = create_refusal_message(case, &refused);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );

        // Nothing was stored: a refused create is not a create.
        let described = refusals.harness.describe(row.name).await;
        assert_eq!(
            described.status,
            404,
            "case {case:?}: a refused create must store nothing, got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );

        // The same row over the wire: the status and code a signed request observes. The
        // catalogue rows are refused by the boundary validator first, which states the
        // same code in its own words, so only the status and the code are pinned here.
        let answer = refusals
            .harness
            .send(
                "POST",
                &format!("/profilingGroups?clientToken=wire-{index}"),
                "application/json",
                serde_json::to_vec(&row.body()).expect("a serializable body"),
            )
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 over the wire, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE} over the wire, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn a_read_refuses_a_bad_window_with_the_shape_its_model_declares() {
    let refusals = Refusals::new();
    create_group(&refusals).await;

    for row in get_rows() {
        let case = row.case;
        let refused = refusals
            .get_profile(row.input())
            .await
            .expect_err(&format!("case {case:?}: this window must be refused"));

        assert_modelled_validation(case, &refused);
        let message = get_profile_refusal_message(case, &refused);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );

        // The same row over the wire, where the window rules are the handler's own — the
        // model states none of them — so the sentence travels too.
        let answer = refusals.harness.get_profile(GROUP, &row.query()).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 over the wire, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE} over the wire, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_message(),
            row.expected,
            "case {case:?}: expected {:?} over the wire, got {:?}",
            row.expected,
            answer.error_message()
        );
    }
}

#[tokio::test]
async fn input_these_operations_accept_is_not_refused() {
    let refusals = Refusals::new();

    // Every entry of the catalogue is accepted, and so is a create naming none at all, with
    // a tag map inside the published limits. What was stored reads back.
    let accepted: Vec<Option<String>> = COMPUTE_PLATFORM_VALUES
        .iter()
        .map(|platform| Some((*platform).to_string()))
        .chain(std::iter::once(None))
        .collect();
    for (index, platform) in accepted.into_iter().enumerate() {
        let case = platform
            .clone()
            .unwrap_or_else(|| "no computePlatform".to_string());
        let name = format!("cgp-accepted-{index}");
        let created = refusals
            .create(CreateProfilingGroupInput {
                profiling_group_name: name.clone(),
                compute_platform: platform.clone(),
                client_token: format!("accepted-{index}"),
                agent_orchestration_config: None,
                tags: Some(tags(&[("team".to_string(), "profiling".to_string())])),
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: must be created, got {error:?}"));
        let expected_platform = platform
            .clone()
            .unwrap_or_else(|| state::DEFAULT_COMPUTE_PLATFORM.to_string());
        assert_eq!(
            created.profiling_group.compute_platform.as_deref(),
            Some(expected_platform.as_str()),
            "case {case:?}: expected {expected_platform:?}, got {:?}",
            created.profiling_group.compute_platform
        );

        // The round trip, through the reader the service answers a description with.
        let described = refusals.harness.describe(&name).await;
        assert_eq!(
            described.status,
            200,
            "case {case:?}: the created group must read back, got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );
        let body = described.json();
        // `profilingGroup` is the operation's `@httpPayload`, so the body IS the
        // description.
        let read_back = body
            .get("computePlatform")
            .and_then(serde_json::Value::as_str);
        assert_eq!(
            read_back,
            Some(expected_platform.as_str()),
            "case {case:?}: expected {expected_platform:?} read back, got {read_back:?}"
        );
        let read_tag = body.pointer("/tags/team").and_then(serde_json::Value::as_str);
        assert_eq!(
            read_tag,
            Some("profiling"),
            "case {case:?}: expected the stored tag read back, got {read_tag:?}"
        );
    }

    // A window the operation's doc allows is not refused on its input: the only thing left
    // to report is the absence of a profile, which is this operation's 404 and not a 400.
    create_group(&refusals).await;
    for (case, start, end, period) in [
        (
            "both bounds, an hour apart",
            Some(DateTime::from_secs(START_SECS)),
            Some(DateTime::from_secs(START_SECS + 3_600)),
            None,
        ),
        (
            "a start and a length",
            Some(DateTime::from_secs(START_SECS)),
            None,
            Some("PT1H"),
        ),
        (
            "an end and a length",
            None,
            Some(DateTime::from_secs(START_SECS)),
            Some("PT1H"),
        ),
        ("a length alone", None, None, Some("PT5M")),
        ("no window member at all", None, None, None),
    ] {
        let answered = refusals
            .get_profile(GetProfileInput {
                profiling_group_name: GROUP.to_string(),
                start_time: start,
                end_time: end,
                period: period.map(str::to_string),
                max_depth: None,
                accept: None,
            })
            .await;
        let error = answered.expect_err(&format!(
            "case {case:?}: the group holds no profile, so this read reports an absence"
        ));
        assert_eq!(
            error.http_status(),
            404,
            "case {case:?}: a window this operation accepts must not be refused as input, got \
             {} ({error})",
            error.http_status()
        );
        assert_ne!(
            error.shape_name(),
            VALIDATION_CODE,
            "case {case:?}: expected a not-found, got {:?}",
            error.shape_name()
        );
    }
}
