// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCodeSigningConfig`: "Update the code signing
//! configuration. Changes to the code signing configuration take effect the next time a
//! user tries to deploy a code package to the function."
//!
//! The rule under test: an update addresses one stored code signing configuration by the
//! ARN it was minted with, replaces exactly the members the request states and leaves
//! every member it omits — and every other configuration — as it was, re-stamping
//! `LastModified` only when something actually changed; the identity it was created with
//! is never re-minted, and an ARN naming no configuration this account holds is answered
//! with the modelled 404 rather than a new record.
//!
//! Every row drives real HTTP requests through the router and reads the configuration
//! back through `GetCodeSigningConfig`, so an update that was answered but never
//! persisted fails here. The rows differ along what the rule generalises over: which
//! subset of the modelled members the request states, the number of signing profiles
//! (including each modelled bound), which `CodeSigningPolicy` value the policy carries,
//! the description (absent, empty and at its modelled maximum) and whether the stated
//! values differ from the stored ones at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where the code-signing-configuration operations are bound, from
/// `smithy_query(query="operation", operation="CreateCodeSigningConfig")`
/// (`POST /2020-04-22/code-signing-configs`),
/// `smithy_query(query="operation", operation="GetCodeSigningConfig")`
/// (`GET /2020-04-22/code-signing-configs/{CodeSigningConfigArn}`) and
/// `smithy_query(query="operation", operation="UpdateCodeSigningConfig")`
/// (`PUT /2020-04-22/code-signing-configs/{CodeSigningConfigArn}`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `ListTags` is bound (`smithy_query(query="operation", operation="ListTags")`:
/// `GET /2017-03-31/tags/{Resource}`), which is how a configuration's tags are read back:
/// the `CodeSigningConfig` shape carries none.
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `CodeSigningPolicy` values the model declares, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`).
const POLICY_WARN: &str = "Warn";
/// See [`POLICY_WARN`].
const POLICY_ENFORCE: &str = "Enforce";

/// The bounds the model puts on `AllowedPublishers$SigningProfileVersionArns`
/// (`@length(min: 1, max: 20)`), from
/// `smithy_query(query="shape", shape="SigningProfileVersionArns")`.
const PROFILES_MIN: usize = 1;
const PROFILES_MAX: usize = 20;

/// The longest `Description` the model admits (`@length(min: 0, max: 256)`), from
/// `smithy_query(query="shape", shape="Description")`.
const DESCRIPTION_MAX: usize = 256;

/// A well-formed `CodeSigningConfigArn` for a configuration this account does not hold:
/// `csc-` and 17 characters, as the member's own pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) spells it.
const ABSENT_ARN: &str =
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0";

/// `LastModified` is reported to whole milliseconds, so two calls inside one millisecond
/// render the same stamp. Each step of the table waits past one millisecond, which makes
/// "a change re-stamps it" and "a no-op leaves it alone" distinguishable.
const STAMP_RESOLUTION: Duration = Duration::from_millis(5);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `CodeSigningConfig` member of the body, which every one of these operations
    /// answers with.
    fn config(&self) -> Value {
        self.json()
            .get("CodeSigningConfig")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A `String` member of the answered `CodeSigningConfig`.
    fn config_string(&self, member: &str) -> Option<String> {
        self.config()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateCodeSigningConfig`.
async fn create_config(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", CODE_SIGNING_CONFIGS_URI, body).await
}

/// `GetCodeSigningConfig` on the configuration `arn` names.
async fn get_config(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `UpdateCodeSigningConfig` on the configuration `arn` names.
async fn update_config(router: &MultiServiceRouter, arn: &str, body: &Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}"),
        body,
    )
    .await
}

/// `ListTags` on the resource `arn` names.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await
}

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form AWS
/// Signer vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// `count` signing profile version ARNs, as a JSON list.
fn signing_profiles(count: usize) -> Value {
    Value::Array((0..count).map(|index| json!(signing_profile(index))).collect())
}

/// The ARN of a code signing configuration of `id`, as the model's
/// `CodeSigningConfigArn` pattern spells it.
fn config_arn(id: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:code-signing-config:{id}")
}

/// The configuration the sequential table starts from: two signing profiles, the stricter
/// policy, a description and tags.
fn starting_request() -> Value {
    json!({
        "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(2) },
        "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
        "Description": "the configuration the updates start from",
        "Tags": { "team": "platform" },
    })
}

/// One step of the update table: the request, and what the rule says the stored
/// configuration then reports.
struct UpdateStep {
    name: &'static str,
    update: Value,
    /// The `SigningProfileVersionArns` the record must then hold, in request order.
    expected_profiles: Vec<String>,
    /// The `UntrustedArtifactOnDeployment` the record must then report.
    expected_policy: &'static str,
    /// The `Description` the record must then report.
    expected_description: Option<String>,
    /// Whether the step changes any stored value, which is what decides whether
    /// `LastModified` moves.
    changes: bool,
}

/// The steps, applied in order to the one configuration [`starting_request`] creates.
/// Each differs from the others in which subset of the modelled members it states, the
/// number of signing profiles, the policy value or the description.
fn update_steps() -> Vec<UpdateStep> {
    let started_from = Some("the configuration the updates start from".to_string());
    let longest = "d".repeat(DESCRIPTION_MAX);
    vec![
        // Only the description: the publishers and the policy stay as created.
        UpdateStep {
            name: "states only the description",
            update: json!({ "Description": "only the description changed" }),
            expected_profiles: (0..2).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some("only the description changed".to_string()),
            changes: true,
        },
        // Only the publishers, at the modelled maximum of the list.
        UpdateStep {
            name: "states only the allowed publishers, at the modelled maximum",
            update: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MAX) },
            }),
            expected_profiles: (0..PROFILES_MAX).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some("only the description changed".to_string()),
            changes: true,
        },
        // Only the policy, switched to the other `CodeSigningPolicy` value.
        UpdateStep {
            name: "states only the policy, switched to the other modelled value",
            update: json!({ "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_WARN } }),
            expected_profiles: (0..PROFILES_MAX).map(signing_profile).collect(),
            expected_policy: POLICY_WARN,
            expected_description: Some("only the description changed".to_string()),
            changes: true,
        },
        // Every member the operation models at once, with the fewest signing profiles the
        // list admits and the longest description.
        UpdateStep {
            name: "states every member at once, with the fewest profiles",
            update: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MIN) },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
                "Description": longest.clone(),
            }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some(longest.clone()),
            changes: true,
        },
        // The same values again: nothing in the record moves, so neither does the stamp.
        UpdateStep {
            name: "re-states exactly the stored values",
            update: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(PROFILES_MIN) },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
                "Description": longest.clone(),
            }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some(longest.clone()),
            changes: false,
        },
        // A request stating no member at all leaves every member alone.
        UpdateStep {
            name: "states no member at all",
            update: json!({}),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some(longest.clone()),
            changes: false,
        },
        // A policy structure whose only member is optional and absent states no policy,
        // so the stored policy stays.
        UpdateStep {
            name: "states a policy structure carrying no policy member",
            update: json!({ "CodeSigningPolicies": {} }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some(longest),
            changes: false,
        },
        // The bottom of the description's modelled length range.
        UpdateStep {
            name: "states an empty description",
            update: json!({ "Description": "" }),
            expected_profiles: (0..PROFILES_MIN).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: Some(String::new()),
            changes: true,
        },
        // Back to a middling number of profiles and the description the create stated:
        // the record is reconfigurable in both directions.
        UpdateStep {
            name: "states three profiles and the description the create stated",
            update: json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(3) },
                "Description": "the configuration the updates start from",
            }),
            expected_profiles: (0..3).map(signing_profile).collect(),
            expected_policy: POLICY_ENFORCE,
            expected_description: started_from,
            changes: true,
        },
    ]
}

/// The members of an answered `CodeSigningConfig` that come out of the stored record, as
/// a comparable value.
fn reported(answer: &Answer) -> Value {
    let config = answer.config();
    json!({
        "CodeSigningConfigId": config["CodeSigningConfigId"].clone(),
        "CodeSigningConfigArn": config["CodeSigningConfigArn"].clone(),
        "Description": config["Description"].clone(),
        "AllowedPublishers": config["AllowedPublishers"].clone(),
        "CodeSigningPolicies": config["CodeSigningPolicies"].clone(),
        "LastModified": config["LastModified"].clone(),
    })
}

/// The policy an answer reports.
fn reported_policy(answer: &Answer) -> Option<String> {
    answer
        .config()
        .get("CodeSigningPolicies")
        .and_then(|policies| policies.get("UntrustedArtifactOnDeployment"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn an_update_rewrites_the_members_it_states_and_leaves_the_rest_alone() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let created = create_config(&router, &starting_request()).await;
    assert_eq!(
        created.status,
        201,
        "expected 201 from CreateCodeSigningConfig, got {} {}",
        created.status,
        created.text()
    );
    let id = created
        .config_string("CodeSigningConfigId")
        .unwrap_or_default();
    let arn = config_arn(&id);
    let mut last_modified = created.config_string("LastModified").unwrap_or_default();

    for step in update_steps() {
        let label = step.name;
        // The stamp is reported to whole milliseconds; wait past one so a step that
        // changes something must report a LATER stamp than the previous step's.
        tokio::time::sleep(STAMP_RESOLUTION).await;

        let updated = update_config(&router, &arn, &step.update).await;
        assert_eq!(
            updated.status,
            200,
            "case {label:?}: expected 200 from UpdateCodeSigningConfig, got {} {}",
            updated.status,
            updated.text()
        );

        // The identity is minted once, at creation, and an update never re-mints it.
        assert_eq!(
            updated.config_string("CodeSigningConfigId"),
            Some(id.clone()),
            "case {label:?}: expected the id {id:?} minted at creation, got {:?}",
            updated.config_string("CodeSigningConfigId")
        );
        assert_eq!(
            updated.config_string("CodeSigningConfigArn"),
            Some(arn.clone()),
            "case {label:?}: expected the ARN {arn:?} minted at creation, got {:?}",
            updated.config_string("CodeSigningConfigArn")
        );

        // The members the step states, and the members it leaves alone.
        let expected_publishers = json!({
            "SigningProfileVersionArns": Value::Array(
                step.expected_profiles.iter().map(|profile| json!(profile)).collect(),
            ),
        });
        assert_eq!(
            updated.config()["AllowedPublishers"],
            expected_publishers,
            "case {label:?}: expected the allowed publishers {expected_publishers}, got {}",
            updated.text()
        );
        assert_eq!(
            reported_policy(&updated).as_deref(),
            Some(step.expected_policy),
            "case {label:?}: expected the policy {:?}, got {:?}",
            step.expected_policy,
            reported_policy(&updated)
        );
        assert_eq!(
            updated.config_string("Description"),
            step.expected_description,
            "case {label:?}: expected the description {:?}, got {:?}",
            step.expected_description,
            updated.config_string("Description")
        );

        // The stamp moves exactly when the record did.
        let stamp = updated.config_string("LastModified").unwrap_or_default();
        if step.changes {
            assert!(
                stamp > last_modified,
                "case {label:?}: a change must re-stamp LastModified past \
                 {last_modified:?}, got {stamp:?}"
            );
        } else {
            assert_eq!(
                stamp, last_modified,
                "case {label:?}: a call that changed nothing must leave LastModified at \
                 {last_modified:?}, got {stamp:?}"
            );
        }
        last_modified = stamp;

        // Read it back: an update that was answered but never persisted fails here, and
        // a read that changed it fails the second read.
        let first = get_config(&router, &arn).await;
        assert_eq!(
            first.status,
            200,
            "case {label:?}: expected 200 from GetCodeSigningConfig on {arn:?}, got {} {}",
            first.status,
            first.text()
        );
        assert_eq!(
            reported(&first),
            reported(&updated),
            "case {label:?}: expected GetCodeSigningConfig to report what the update \
             persisted, {}, got {}",
            updated.text(),
            first.text()
        );
        let second = get_config(&router, &arn).await;
        assert_eq!(
            reported(&second),
            reported(&first),
            "case {label:?}: two reads of an unchanged configuration report the same \
             thing, got {} then {}",
            first.text(),
            second.text()
        );

        // The tags are not a member this operation models, so they survive every update.
        let tags = list_tags(&router, &arn).await;
        assert_eq!(
            tags.json().get("Tags").cloned().unwrap_or(json!({})),
            json!({ "team": "platform" }),
            "case {label:?}: expected the tags the create stated to survive the update, \
             got {}",
            tags.text()
        );
    }

    // Every step reconfigured the SAME record: none of them created a second one.
    let stored = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .code_signing_configs
        .len();
    assert_eq!(
        stored, 1,
        "expected the updates to reconfigure one stored configuration, got {stored}"
    );
}

#[tokio::test]
async fn an_update_of_a_code_signing_config_this_account_does_not_hold_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let update = json!({
        "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
        "Description": "an update of nothing",
    });

    // Nothing has been created, so even a well-formed ARN names no configuration.
    let missing = update_config(&router, ABSENT_ARN, &update).await;
    assert_eq!(
        missing.status,
        404,
        "case {ABSENT_ARN:?}: expected 404 from UpdateCodeSigningConfig on a configuration \
         this account does not hold, got {} {}",
        missing.status,
        missing.text()
    );
    assert!(
        missing.error_code().contains("ResourceNotFoundException"),
        "case {ABSENT_ARN:?}: expected ResourceNotFoundException, got {}",
        missing.error_code()
    );

    // The refusal persisted nothing: the ARN still names no configuration.
    let absent = get_config(&router, ABSENT_ARN).await;
    assert_eq!(
        absent.status,
        404,
        "case {ABSENT_ARN:?}: a refused update must create nothing, got {} {}",
        absent.status,
        absent.text()
    );

    // One configuration exists; a DIFFERENT well-formed ARN still names none of it, and
    // the configuration that does exist is untouched.
    let created = create_config(&router, &starting_request()).await;
    assert_eq!(
        created.status,
        201,
        "expected 201 from CreateCodeSigningConfig, got {} {}",
        created.status,
        created.text()
    );
    let arn = config_arn(
        &created
            .config_string("CodeSigningConfigId")
            .unwrap_or_default(),
    );
    let still_missing = update_config(&router, ABSENT_ARN, &update).await;
    assert_eq!(
        still_missing.status,
        404,
        "case {ABSENT_ARN:?}: expected 404 even once another configuration exists, got {} {}",
        still_missing.status,
        still_missing.text()
    );
    let untouched = get_config(&router, &arn).await;
    assert_eq!(
        reported(&untouched),
        reported(&created),
        "case {ABSENT_ARN:?}: an update of an absent ARN must leave the configuration that \
         does exist alone, expected {}, got {}",
        created.text(),
        untouched.text()
    );

    // A value that is no code-signing-config ARN at all is a client refusal, never an
    // update of some other configuration.
    let malformed = update_config(&router, "not-a-code-signing-config-arn", &update).await;
    assert_eq!(
        malformed.status,
        400,
        "case \"not-a-code-signing-config-arn\": expected 400 from \
         UpdateCodeSigningConfig, got {} {}",
        malformed.status,
        malformed.text()
    );
}

#[tokio::test]
async fn an_update_touches_only_the_configuration_it_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Three configurations created from the same request shape, so only the ARN tells
    // them apart.
    let mut arns: Vec<String> = Vec::new();
    let mut before: Vec<Value> = Vec::new();
    for round in 0..3 {
        let created = create_config(
            &router,
            &json!({
                "AllowedPublishers": { "SigningProfileVersionArns": signing_profiles(1) },
                "Description": format!("round {round}"),
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {round}: expected 201 from CreateCodeSigningConfig, got {} {}",
            created.status,
            created.text()
        );
        arns.push(
            created
                .config_string("CodeSigningConfigArn")
                .unwrap_or_default(),
        );
        before.push(reported(&created));
    }

    // Update each one in turn, checking after every update that the OTHERS still report
    // exactly what they did before it.
    for (round, arn) in arns.iter().enumerate() {
        tokio::time::sleep(STAMP_RESOLUTION).await;
        let updated = update_config(
            &router,
            arn,
            &json!({
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": POLICY_ENFORCE },
                "Description": format!("round {round} updated"),
            }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {round}: expected 200 from UpdateCodeSigningConfig on {arn:?}, got {} {}",
            updated.status,
            updated.text()
        );
        before[round] = reported(&updated);

        for (other, other_arn) in arns.iter().enumerate() {
            let fetched = get_config(&router, other_arn).await;
            assert_eq!(
                reported(&fetched),
                before[other],
                "case {round}: expected {other_arn:?} to still report {}, got {}",
                before[other],
                fetched.text()
            );
        }
    }

    let stored = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .code_signing_configs
        .len();
    assert_eq!(
        stored, 3,
        "expected three stored code signing configurations after the updates, got {stored}"
    );
}
