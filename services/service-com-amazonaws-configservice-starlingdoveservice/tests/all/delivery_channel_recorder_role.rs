// SPDX-License-Identifier: CC0-1.0

//! What the CONFIGURATION RECORDER'S IAM ROLE decides about a delivery channel.
//!
//! The rule under test: **Config makes a delivery as its own service principal under the
//! configuration recorder's IAM role, so `PutDeliveryChannel` takes a channel only while
//! IAM shows that role can be assumed by `config.amazonaws.com`. Where IAM answers that it
//! cannot — this account holds no role under the name the ARN carries, or the trust
//! document the role carries does not admit that principal — the call is refused with HTTP
//! 400 `InsufficientDeliveryPolicyException`, worded after that assume and naming the
//! bucket and the role, and NOTHING is persisted: the channel-existence reads
//! (`DescribeDeliveryChannels`, `DescribeDeliveryChannelStatus`, `DeleteDeliveryChannel`)
//! then answer 400 `NoSuchDeliveryChannelException` quoting the name the refused put
//! named. A role nothing can be shown about is refused nothing.**
//!
//! Config and IAM are mounted on ONE engine, because the assume is put to IAM itself over
//! the emulator's own in-process callback: that is why every other suite in this crate,
//! which mounts Config alone, still puts delivery channels whose recorder role no IAM
//! record backs.
//!
//! The rows differ along what the rule generalises over: what the role's trust document
//! says (which principal, which action, which effect, how many statements, whether it is
//! readable as a policy at all, whether the role carries one), whether this account holds
//! the role, whose account the ARN names, the path IAM holds the role under, and the
//! channel and bucket names the refusal has to quote back.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    DEFAULT_PATH,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The Config service principal a role has to admit for Config to deliver under it.
const CONFIG_PRINCIPAL: &str = "config.amazonaws.com";

/// The error code a delivery Config cannot make is refused with, and the one every
/// existence read answers for a channel this account does not hold.
const INSUFFICIENT_POLICY: &str = "InsufficientDeliveryPolicyException";
/// See [`INSUFFICIENT_POLICY`].
const NO_SUCH_CHANNEL: &str = "NoSuchDeliveryChannelException";

/// Config, IAM, and optionally Amazon S3, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    callback: EmbeddedEmulator,
}

impl Emulator {
    /// Config and IAM alone: with no S3 to ask, no bucket policy is established for any
    /// bucket, so the ROLE is the only thing that can decide a put.
    fn new() -> Self {
        Self::build(false)
    }

    /// The same, with S3 mounted: a bucket S3 holds answers about its own policy, so both
    /// halves of the decision are live and their ORDER is observable.
    fn with_s3() -> Self {
        Self::build(true)
    }

    fn build(with_s3: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        if with_s3 {
            router.register_service(
                service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// One Config call (`awsJson1_1`), answering the HTTP status and the parsed body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/config/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("config.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A Config call that must answer 200.
    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Create `bucket` through S3's own API, carrying no policy at all.
    async fn create_bucket(&self, bucket: &str) {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
            .force_path_style(true)
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_s3::Client::from_conf(config)
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
    }

    /// Seed `role_name` into the shared engine's IAM store under `path`, carrying `trust`
    /// as its `AssumeRolePolicyDocument` — the record `CreateRole` writes — and answer the
    /// ARN IAM holds it under.
    fn seed_role(&self, path: &str, role_name: &str, trust: Option<String>) -> String {
        let state = self.engine.state::<IamState>();
        let mut store = state.lock();
        let arn = role_arn(path, role_name);
        store.roles.insert(
            name_key(role_name),
            Role {
                path: path.to_string(),
                role_name: role_name.to_string(),
                role_id: format!("AROA{}", role_name.to_uppercase()),
                arn: arn.clone(),
                create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
                assume_role_policy_document: trust,
                description: None,
                max_session_duration: None,
                permissions_boundary: None,
                tags: None,
                role_last_used: None,
                source_role_template: None,
            },
        );
        arn
    }
}

/// `body`'s error code, as the awsJson envelope carries it.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// `body`'s error message.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the refusal must carry a message, got {body}"))
        .to_string()
}

/// A trust document with one statement: `effect` for the service `principal` performing
/// `action`.
fn trust(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// The trust document a role Config can assume carries.
fn config_trust() -> String {
    trust("Allow", CONFIG_PRINCIPAL, "sts:AssumeRole")
}

/// The sentence a delivery the recorder's role withholds is refused with.
fn unable_to_assume(bucket: &str, role: &str) -> String {
    format!("Insufficient delivery policy to s3 bucket: {bucket}, unable to assume role: {role}.")
}

/// The sentence every existence read answers for a channel this account does not hold.
fn cannot_find_channel(channel: &str) -> String {
    format!("Cannot find delivery channel with specified name '{channel}'.")
}

/// Put the customer managed configuration recorder a delivery channel needs, running as
/// `role`.
async fn with_recorder(emulator: &Emulator, role: &str) {
    emulator
        .ok(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": "the-recorder",
                    "roleARN": role,
                    "recordingGroup": { "allSupported": true },
                }
            }),
        )
        .await;
}

/// The `PutDeliveryChannel` body for `channel` delivering into `bucket`.
fn channel_request(channel: &str, bucket: &str) -> Value {
    json!({ "DeliveryChannel": { "name": channel, "s3BucketName": bucket } })
}

/// Assert that this account holds NO delivery channel under `channel`: every read that
/// names it answers the modelled 400, quoting the name the request carried.
async fn assert_no_such_channel(emulator: &Emulator, case: &str, channel: &str) {
    let expected = cannot_find_channel(channel);
    let reads: [(&str, Value); 3] = [
        (
            "DescribeDeliveryChannels",
            json!({ "DeliveryChannelNames": [channel] }),
        ),
        (
            "DescribeDeliveryChannelStatus",
            json!({ "DeliveryChannelNames": [channel] }),
        ),
        (
            "DeleteDeliveryChannel",
            json!({ "DeliveryChannelName": channel }),
        ),
    ];
    for (operation, body) in reads {
        let (status, answered) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answered), error_message(&answered)),
            (400, NO_SUCH_CHANNEL.to_string(), expected.clone()),
            "case {case:?} ({operation}): expected HTTP 400 {NO_SUCH_CHANNEL} and \
             {expected:?}, got {status} with {answered}"
        );
    }
    // The unnamed read reports the whole account, which holds nothing either.
    let listed = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
    assert_eq!(
        listed["DeliveryChannels"],
        json!([]),
        "case {case:?}: a refused put persists no channel, got {listed}"
    );
}

/// One row of the table: what IAM holds under the row's role name, and whether the rule
/// says Config must take the channel.
struct Row {
    case: &'static str,
    /// The trust document the account's role carries, or `None` for a row whose role the
    /// account does not hold at all.
    held: Option<Option<String>>,
    /// Whether the put must be admitted.
    admitted: bool,
}

/// A delivery channel is taken only while IAM shows Config can assume the configuration
/// recorder's role — and a refusal names the bucket and that role, and persists nothing.
#[tokio::test]
async fn a_channel_is_taken_only_while_config_can_assume_the_recorders_role() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a trust document admitting the Config principal",
            held: Some(Some(config_trust())),
            admitted: true,
        },
        Row {
            case: "a service wildcard in the action",
            held: Some(Some(trust("Allow", CONFIG_PRINCIPAL, "sts:*"))),
            admitted: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            held: Some(Some(trust("Allow", "CONFIG.amazonaws.com", "STS:AssumeRole"))),
            admitted: true,
        },
        Row {
            case: "a document whose second statement admits the Config principal",
            held: Some(Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": CONFIG_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
                .to_string(),
            )),
            admitted: true,
        },
        Row {
            case: "a trust document admitting another service only",
            held: Some(Some(trust(
                "Allow",
                "cloudtrail.amazonaws.com",
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document naming another action",
            held: Some(Some(trust("Allow", CONFIG_PRINCIPAL, "s3:GetObject"))),
            admitted: false,
        },
        Row {
            case: "a trust document explicitly denying the Config principal",
            held: Some(Some(trust("Deny", CONFIG_PRINCIPAL, "sts:AssumeRole"))),
            admitted: false,
        },
        Row {
            case: "a trust document carrying no statement",
            held: Some(Some(json!({ "Version": "2012-10-17" }).to_string())),
            admitted: false,
        },
        Row {
            case: "a trust document that is not a policy at all",
            held: Some(Some("this is not a policy".to_string())),
            admitted: false,
        },
        Row {
            case: "a role carrying no trust document",
            held: Some(None),
            admitted: false,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            admitted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Each row stands its own emulator up, and names its own channel and bucket, so
        // the refusal's variable parts are the ones THAT row sent.
        let emulator = Emulator::new();
        let role_name = format!("delivery-role-{index}");
        let channel = format!("delivery-channel-{index}");
        let bucket = format!("delivery-bucket-{index}");
        let role = match &row.held {
            Some(trust) => emulator.seed_role(DEFAULT_PATH, &role_name, trust.clone()),
            // Not seeded, so IAM answers that this account holds no such role. The ARN is
            // the one IAM would hold it under.
            None => role_arn(DEFAULT_PATH, &role_name),
        };
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        let case = row.case;
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be admitted, got {status} with {body}"
            );
            // Round trip: the channel the assume admitted is readable, and its status is
            // reported, under the name it was put with.
            let read = emulator
                .ok(
                    "DescribeDeliveryChannels",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            assert_eq!(
                read["DeliveryChannels"][0]["s3BucketName"],
                json!(bucket),
                "case {case:?}: expected the held channel to name the bucket it was put \
                 with, got {read}"
            );
            let reported = emulator
                .ok(
                    "DescribeDeliveryChannelStatus",
                    &json!({ "DeliveryChannelNames": [&channel] }),
                )
                .await;
            assert_eq!(
                reported["DeliveryChannelsStatus"][0]["name"],
                json!(channel),
                "case {case:?}: expected the status of the channel that was put, got \
                 {reported}"
            );
            continue;
        }

        let expected = unable_to_assume(&bucket, &role);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The same decision over the forms the recorder's `roleARN` may name a role by: a role
/// under a path is held by IAM under its NAME, so it is judged like one at the default
/// path — while an ARN of ANOTHER account names a role this emulator's IAM cannot be asked
/// about, so nothing is shown and nothing is refused.
#[tokio::test]
async fn the_recorders_role_is_resolved_whatever_arn_names_it() {
    // (case, how the row's role ARN is built from the role it seeds, whether admitted)
    let rows: [(&str, bool, Option<&str>, bool); 5] = [
        ("this account's role, trusting Config", true, None, true),
        (
            "the same role under a service-role path, which IAM holds by name",
            true,
            Some("/service-role/"),
            true,
        ),
        (
            "this account's role, trusting another service",
            false,
            None,
            false,
        ),
        (
            "that same role under a service-role path",
            false,
            Some("/service-role/"),
            false,
        ),
        // Another account's IAM is not this emulator's to ask, so nothing is shown about
        // the role and the put goes ahead.
        (
            "another account's role ARN, which this IAM cannot be asked about",
            false,
            Some("other-account"),
            true,
        ),
    ];

    for (index, (case, trusts_config, path, admitted)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let role_name = format!("role-form-{index}");
        let channel = format!("channel-form-{index}");
        let bucket = format!("bucket-form-{index}");
        let document = if *trusts_config {
            config_trust()
        } else {
            trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")
        };
        let role = match path {
            Some("other-account") => {
                format!("arn:aws:iam::123456789012:role/{role_name}")
            }
            Some(path) => emulator.seed_role(path, &role_name, Some(document)),
            None => emulator.seed_role(DEFAULT_PATH, &role_name, Some(document)),
        };
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the put to be admitted, got {status} with {body}"
            );
            continue;
        }
        let expected = unable_to_assume(&bucket, &role);
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}

/// The assume is made BEFORE anything of the bucket is read, so a put that fails both
/// halves at once reports the ROLE: with S3 mounted and a bucket carrying no policy — which
/// on its own denies the first S3 call Config makes — the sentence names the assume while
/// the role withholds it, and the bucket call once the role admits it.
///
/// The rows differ along whether the role is assumable, which is what makes the first row
/// about precedence and not about the bucket check having gone away.
#[tokio::test]
async fn the_recorders_role_is_judged_before_the_buckets_policy() {
    // (case, the trust document the seeded role carries, the `unable to …` clause)
    let rows: [(&str, bool, &str); 2] = [
        (
            "a role Config cannot assume, pointed at a bucket with no policy",
            false,
            "assume role",
        ),
        (
            "a role Config can assume, pointed at the same bucket",
            true,
            "get s3 bucket acl",
        ),
    ];

    for (index, (case, assumable, clause)) in rows.iter().enumerate() {
        let emulator = Emulator::with_s3();
        let role_name = format!("ordering-role-{index}");
        let channel = format!("ordering-channel-{index}");
        let bucket = format!("ordering-bucket-{index}");
        emulator.create_bucket(&bucket).await;
        let document = if *assumable {
            config_trust()
        } else {
            trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")
        };
        let role = emulator.seed_role(DEFAULT_PATH, &role_name, Some(document));
        with_recorder(&emulator, &role).await;

        let (status, body) = emulator
            .call("PutDeliveryChannel", &channel_request(&channel, &bucket))
            .await;
        let expected = if *assumable {
            format!("Insufficient delivery policy to s3 bucket: {bucket}, unable to {clause}.")
        } else {
            unable_to_assume(&bucket, &role)
        };
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, INSUFFICIENT_POLICY.to_string(), expected.clone()),
            "case {case:?}: expected HTTP 400 {INSUFFICIENT_POLICY} and {expected:?}, got \
             {status} with {body}"
        );
        assert_no_such_channel(&emulator, case, &channel).await;
    }
}
