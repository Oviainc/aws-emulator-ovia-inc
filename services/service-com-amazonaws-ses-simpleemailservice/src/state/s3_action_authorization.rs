// SPDX-License-Identifier: CC0-1.0

//! Whether the Amazon S3 destination a receipt rule's `S3Action` names is one Amazon SES
//! may really write the received message into — the pre-flight every operation that STORES
//! such a rule runs before it persists anything.
//!
//! `S3Action` documents the requirement: "To enable Amazon SES to write emails to your
//! Amazon S3 bucket, use an Amazon Web Services KMS key to encrypt your emails, or publish
//! to an Amazon SNS topic of another account, Amazon SES must have permission to access
//! those resources." The permission is the DESTINATION'S own resource policy, read for the
//! service principal Amazon SES acts under ([`SES_SERVICE_PRINCIPAL`]) and scoped to the
//! receipt rule being stored ([`receipt_rule_arn`]), which is how such a policy is written.
//!
//! So an `S3Action` carries TWO authorizations, and the one modelled error class covers both
//! — `InvalidS3ConfigurationException` is documented as "Indicates that the provided Amazon
//! S3 bucket or Amazon Web Services KMS encryption key is invalid, or that Amazon SES could
//! not publish to the bucket, possibly due to permissions issues":
//!
//! 1. the BUCKET has to exist and its bucket policy has to grant Amazon SES the object write
//!    the action performs ([`OBJECT_WRITE_ACTION`]);
//! 2. the KMS KEY, when the action names one, has to grant Amazon SES the encrypt operations
//!    it seals the message with ([`KEY_ENCRYPT_ACTIONS`]) — except for a key Amazon Web
//!    Services manages, of which `S3Action$KmsKeyArn` says "If you use the Amazon Web
//!    Services managed key, you don't need to perform any extra steps to give Amazon SES
//!    permission to use the key."
//!
//! A bucket that is not there and a destination Amazon SES may not write to are told APART,
//! because a client catching this error class has to know which of the two to go and fix:
//! [`no_such_bucket_message`] and [`could_not_write_message`].
//!
//! Each question is answered by the service that OWNS the answer — Amazon S3 for the bucket
//! and its policy, Amazon Web Services KMS for the key, its manager and its key policy —
//! reached the way the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): the AWS SDK over the emulator's
//! own in-process transport, so nothing here opens a socket or leaves the host. The decision
//! itself is [`::service_common::iam_authorization`], the tree's one evaluation of policy
//! documents against `(action, resource, principal)`.
//!
//! An answer that is not one of those services stating the destination's own facts — an
//! emulator with no object store or no KMS mounted beside this one, a refusal, any other
//! failure to ask — establishes NOTHING, so it refuses nothing: a refusal has to be grounded
//! in what the owning service answered. Both calls READ, so a refused rule leaves the
//! destination exactly as the caller left it.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The service principal Amazon SES acts under when it writes a received message into the
/// bucket a receipt rule names, and therefore the principal a bucket policy or a key policy
/// names to grant it.
pub const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The Amazon S3 action an `S3Action` performs: it SAVES the received message into the
/// bucket ("this action saves the received message to an Amazon Simple Storage Service
/// (Amazon S3) bucket", `S3Action`), which is an object write.
pub const OBJECT_WRITE_ACTION: &str = "s3:PutObject";

/// The object the pre-flight authorizes the write on: Amazon SES writes its own setup
/// notification into the bucket under the action's `ObjectKeyPrefix` when the rule is
/// created, which is the object whose write proves the grant.
///
/// Naming one object rather than the whole bucket is what makes a grant scoped to a PREFIX
/// decide correctly: a policy granting the write under one prefix does not grant it under
/// another.
const SETUP_NOTIFICATION_OBJECT: &str = "AMAZON_SES_SETUP_NOTIFICATION";

/// The KMS operations Amazon SES needs the action's key for: `S3Action$KmsKeyArn` is "the
/// customer managed key that Amazon SES should use to encrypt your emails before saving them
/// to the Amazon S3 bucket", and encrypting a message under a KMS key is generating a data
/// key under it and encrypting with it.
pub const KEY_ENCRYPT_ACTIONS: &[&str] = &["kms:Encrypt", "kms:GenerateDataKey"];

/// The condition key naming the ACCOUNT whose resource a service principal is acting on
/// behalf of, which is how a grant to such a principal is scoped to one account.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The condition key naming the RESOURCE a service principal is acting on behalf of, which
/// for this pre-flight is the receipt rule being stored ([`receipt_rule_arn`]).
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key naming the service a KMS request was made THROUGH, whose value is that
/// service's regional endpoint name.
const VIA_SERVICE_KEY: &str = "kms:ViaService";

/// The condition key naming the account the KMS request was made under.
const CALLER_ACCOUNT_KEY: &str = "kms:CallerAccount";

/// The Amazon S3 error code for a bucket the object store does not hold.
const NO_SUCH_BUCKET_CODE: &str = "NoSuchBucket";

/// The Amazon S3 error code for a bucket that exists and carries no policy.
const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";

/// How many DISTINCT Amazon S3 destinations one stored rule is authorized against.
///
/// An EMULATOR ceiling, not an AWS quota: `ReceiptRule$Actions` is a list a caller writes, and
/// each destination in it costs this pre-flight a read of two other services, so the work one
/// request can drive is bounded here. A receipt rule's action list is short — it is the
/// ordered set of things SES does with one received message — so no rule a client really
/// writes reaches it.
pub const MAX_AUTHORIZED_DESTINATIONS: usize = 20;

/// Why the `S3Action` of a receipt rule cannot be stored: the bucket the refusal names (the
/// one the request itself carried, which is what `InvalidS3ConfigurationException$Bucket`
/// reports) and the sentence that says which of the two faults it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ActionRefusal {
    /// `InvalidS3ConfigurationException$Bucket`: the action's own `BucketName`.
    pub bucket: String,
    /// The refusal's message.
    pub message: String,
}

impl S3ActionRefusal {
    /// The refusal of `bucket`, worded by `message`.
    fn of(bucket: &str, message: String) -> Self {
        Self {
            bucket: bucket.to_string(),
            message,
        }
    }
}

/// How Amazon SES words the refusal of an `S3Action` naming a bucket that is not there: the
/// fixed sentence plus the `BucketName` the request carried.
#[must_use]
pub fn no_such_bucket_message(bucket: &str) -> String {
    format!("No such bucket: {bucket}")
}

/// How Amazon SES words the refusal of an `S3Action` whose destination it may not write the
/// message into — the bucket's own policy grants it nothing, or the key it would encrypt the
/// message with does. The message names the BUCKET either way, which is the destination the
/// action addresses.
#[must_use]
pub fn could_not_write_message(bucket: &str) -> String {
    format!("Could not write to bucket: {bucket}")
}

/// The ARN of the receipt rule `rule_name` of the rule set `rule_set_name`, which is the
/// value `aws:SourceArn` carries in a grant written for one receipt rule.
#[must_use]
pub fn receipt_rule_arn(region: &str, rule_set_name: &str, rule_name: &str) -> String {
    format!(
        "arn:aws:ses:{region}:{}:receipt-rule-set/{rule_set_name}:receipt-rule/{rule_name}",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN of the object `key` of `bucket`: an Amazon S3 object ARN leaves the Region and
/// account-id fields empty, because a bucket name is globally unique.
fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The refusal the FIRST `S3Action` of `actions` that Amazon SES may not write to earns, and
/// `None` when every one of them names a destination it may — or when nothing establishes
/// otherwise (see the module docs).
///
/// `rule_set_name` and `rule_name` name the rule being stored, which is what a grant in the
/// destination's policy is scoped to; `region` is the Region the request is signed for, which
/// is the endpoint the bucket and the key are asked of.
///
/// Each DISTINCT destination the rule names is asked about once, and at most
/// [`MAX_AUTHORIZED_DESTINATIONS`] of them are: the actions a rule carries are a caller's
/// list, so the number of asks one request can drive is bounded here rather than by the size
/// of the request. A destination past that ceiling is left unjudged, which is the narrower
/// answer — nothing is refused on an answer nobody asked for.
///
/// The ONE reading, so `CreateReceiptRule` and `UpdateReceiptRule` hold a stored rule's
/// destination to the same requirements instead of each keeping its own copy of them.
pub async fn unauthorized_s3_action(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    rule_set_name: &str,
    rule_name: &str,
    actions: &[crate::ops::types::ReceiptAction],
) -> Option<S3ActionRefusal> {
    let rule_arn = receipt_rule_arn(region, rule_set_name, rule_name);
    let mut asked: Vec<(&str, &str, &str)> = Vec::new();
    for s3_action in actions.iter().filter_map(|action| action.s3_action.as_ref()) {
        let destination = (
            s3_action.bucket_name.as_str(),
            s3_action.object_key_prefix.as_deref().unwrap_or_default(),
            s3_action.kms_key_arn.as_deref().unwrap_or_default(),
        );
        if asked.contains(&destination) {
            continue;
        }
        if asked.len() >= MAX_AUTHORIZED_DESTINATIONS {
            break;
        }
        asked.push(destination);
        if let Some(refusal) = bucket_refusal(ctx, region, s3_action, &rule_arn).await {
            return Some(refusal);
        }
        if let Some(refusal) = key_refusal(ctx, region, s3_action, &rule_arn).await {
            return Some(refusal);
        }
    }
    None
}

/// What asking the object store for a bucket's policy answered.
enum BucketPolicy {
    /// The policy document the bucket carries.
    Document(String),
    /// The bucket is there and carries no policy, so nothing grants Amazon SES the write.
    WithoutPolicy,
    /// There is no such bucket: the write has nowhere to go.
    NoSuchBucket,
    /// The question did not get through, so nothing is known about the bucket.
    Unanswered,
}

/// The refusal the action's BUCKET earns, read out of the object store's own account of it.
async fn bucket_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    s3_action: &crate::ops::types::S3Action,
    rule_arn: &str,
) -> Option<S3ActionRefusal> {
    let bucket = s3_action.bucket_name.as_str();
    match bucket_policy(ctx, region, bucket).await {
        BucketPolicy::NoSuchBucket => {
            Some(S3ActionRefusal::of(bucket, no_such_bucket_message(bucket)))
        }
        BucketPolicy::WithoutPolicy => {
            Some(S3ActionRefusal::of(bucket, could_not_write_message(bucket)))
        }
        BucketPolicy::Document(policy) => {
            let prefix = s3_action.object_key_prefix.as_deref().unwrap_or_default();
            (!bucket_policy_permits_write(&policy, bucket, prefix, rule_arn))
                .then(|| S3ActionRefusal::of(bucket, could_not_write_message(bucket)))
        }
        BucketPolicy::Unanswered => None,
    }
}

/// The policy of the bucket an `S3Action` names, read from this emulator's own object store.
async fn bucket_policy(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> BucketPolicy {
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let failure = match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(answer) => {
            return answer
                .policy
                .map_or(BucketPolicy::WithoutPolicy, BucketPolicy::Document)
        }
        Err(failure) => failure,
    };
    // Amazon S3's own two codes for this read are its answers ABOUT the bucket: it holds no
    // such bucket, or the bucket carries no policy. Any other answer — a refusal, a fault, or
    // no response at all — settles nothing, so nothing is read into it.
    match ::aws_sdk_s3::error::ProvideErrorMetadata::code(&failure) {
        Some(NO_SUCH_BUCKET_CODE) => BucketPolicy::NoSuchBucket,
        Some(NO_SUCH_BUCKET_POLICY_CODE) => BucketPolicy::WithoutPolicy,
        other => {
            ::tracing::warn!(
                bucket,
                code = ?other,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the receipt rule's destination bucket could not be read, so its policy is \
                 not judged"
            );
            BucketPolicy::Unanswered
        }
    }
}

/// Whether `policy` grants Amazon SES the object write this action performs, on the object
/// the pre-flight writes under the action's `prefix` ([`SETUP_NOTIFICATION_OBJECT`]).
fn bucket_policy_permits_write(policy: &str, bucket: &str, prefix: &str, rule_arn: &str) -> bool {
    let object = object_arn(bucket, &format!("{prefix}{SETUP_NOTIFICATION_OBJECT}"));
    let principals = [SES_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        (SOURCE_ARN_KEY.to_string(), rule_arn.to_string()),
    ];
    let request = iam::Request::new(OBJECT_WRITE_ACTION, &object, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ACCOUNT_KEY, SOURCE_ARN_KEY]);
    iam::resource_decision(policy, &request) == Decision::Allowed
}

/// What asking KMS about the key an `S3Action` names answered.
enum KeyPolicy {
    /// The key's own ARN and the key policy it carries, for a key the customer manages.
    Document { key_arn: String, policy: String },
    /// A key Amazon Web Services manages, whose policy it writes for the service that uses
    /// the key: the caller adds no statement of its own to it.
    AwsManaged,
    /// KMS holds no such key, or the identifier names none it can resolve: there is no key
    /// policy to grant Amazon SES anything.
    NoSuchKey,
    /// The question did not get through, so nothing is known about the key.
    Unanswered,
}

/// The refusal the action's KMS KEY earns, read out of KMS's own account of it.
///
/// `None` for an action naming no key at all, for a key Amazon Web Services manages, for a
/// key whose key policy grants Amazon SES its encrypt operations, and for a question KMS did
/// not answer.
async fn key_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    s3_action: &crate::ops::types::S3Action,
    rule_arn: &str,
) -> Option<S3ActionRefusal> {
    let key_id = s3_action.kms_key_arn.as_deref().map(str::trim)?;
    if key_id.is_empty() {
        return None;
    }
    let bucket = s3_action.bucket_name.as_str();
    match key_policy(ctx, region, key_id).await {
        KeyPolicy::AwsManaged | KeyPolicy::Unanswered => None,
        KeyPolicy::NoSuchKey => {
            Some(S3ActionRefusal::of(bucket, could_not_write_message(bucket)))
        }
        KeyPolicy::Document { key_arn, policy } => {
            (!key_policy_permits_encryption(&policy, &key_arn, region, rule_arn))
                .then(|| S3ActionRefusal::of(bucket, could_not_write_message(bucket)))
        }
    }
}

/// What KMS holds for the key `key_id` names, read from this emulator's own KMS.
///
/// Two reads, because the decision needs both: WHO manages the key (`DescribeKey`, which is
/// also what resolves an Amazon Web Services managed key from a predefined alias) and the key
/// policy itself (`GetKeyPolicy`).
async fn key_policy(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> KeyPolicy {
    let client = kms_client(ctx, region);
    let described = match client.describe_key().key_id(key_id).send().await {
        Ok(described) => described,
        Err(failure) => {
            return match failure.as_service_error() {
                // KMS answered that this account and Region hold no such key, or that the
                // identifier is not one it can resolve at all.
                Some(
                    ::aws_sdk_kms::operation::describe_key::DescribeKeyError::NotFoundException(_)
                    | ::aws_sdk_kms::operation::describe_key::DescribeKeyError::InvalidArnException(
                        _,
                    ),
                ) => KeyPolicy::NoSuchKey,
                _ => unanswered_key(key_id, &failure),
            }
        }
    };
    let metadata = described.key_metadata();
    // "The manager of the KMS key. KMS keys in your Amazon Web Services account are either
    // customer managed or Amazon Web Services managed" (`KeyMetadata$KeyManager`): the policy
    // of a key Amazon Web Services manages is written by Amazon Web Services for the service
    // that uses it, so no statement of the caller's is asked for.
    if metadata.and_then(|metadata| metadata.key_manager())
        == Some(&::aws_sdk_kms::types::KeyManagerType::Aws)
    {
        return KeyPolicy::AwsManaged;
    }
    // A key policy's `Resource` element is `*`, which means this KMS key, so the key's own ARN
    // is what the decision names as its resource.
    let Some(key_arn) = metadata.and_then(|metadata| metadata.arn()).map(str::to_string) else {
        return KeyPolicy::Unanswered;
    };
    match client.get_key_policy().key_id(key_id).send().await {
        Ok(answer) => match answer.policy {
            Some(policy) => KeyPolicy::Document { key_arn, policy },
            None => KeyPolicy::Unanswered,
        },
        Err(failure) => match failure.as_service_error() {
            Some(
                ::aws_sdk_kms::operation::get_key_policy::GetKeyPolicyError::NotFoundException(_)
                | ::aws_sdk_kms::operation::get_key_policy::GetKeyPolicyError::InvalidArnException(
                    _,
                ),
            ) => KeyPolicy::NoSuchKey,
            _ => unanswered_key(key_id, &failure),
        },
    }
}

/// Note that KMS did not state the key's facts, and so that the key is judged against no
/// policy. Worth a line to an operator, because a deployment with no KMS to ask is one where
/// this half of the pre-flight does not run; the caller's own value is rendered in its
/// escaped `Debug` form, so an identifier carrying a newline cannot forge a log line.
fn unanswered_key<E: ::std::error::Error + 'static, R: ::std::fmt::Debug>(
    key_id: &str,
    failure: &::aws_sdk_kms::error::SdkError<E, R>,
) -> KeyPolicy {
    ::tracing::warn!(
        kms_key = ?key_id,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(failure),
        "the receipt rule's encryption key could not be read, so its key policy is not judged"
    );
    KeyPolicy::Unanswered
}

/// Whether `policy` lets Amazon SES encrypt the message under the key `key_arn` names.
///
/// Permitted when the key policy grants the service principal at least one of
/// [`KEY_ENCRYPT_ACTIONS`] and denies none of them: a policy that states nothing about this
/// principal grants it nothing ("ensure that you add a statement to your key's policy to give
/// Amazon SES permission to use it", `S3Action$KmsKeyArn`), and an explicit deny refuses
/// whatever else the document allows.
fn key_policy_permits_encryption(
    policy: &str,
    key_arn: &str,
    region: &str,
    rule_arn: &str,
) -> bool {
    let principals = [SES_SERVICE_PRINCIPAL.to_string()];
    let context = [
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        (SOURCE_ARN_KEY.to_string(), rule_arn.to_string()),
        (
            VIA_SERVICE_KEY.to_string(),
            format!("ses.{region}.amazonaws.com"),
        ),
        (
            CALLER_ACCOUNT_KEY.to_string(),
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
    ];
    let published = [
        SOURCE_ACCOUNT_KEY,
        SOURCE_ARN_KEY,
        VIA_SERVICE_KEY,
        CALLER_ACCOUNT_KEY,
    ];
    let mut granted = false;
    for action in KEY_ENCRYPT_ACTIONS {
        let request = iam::Request::new(action, key_arn, &principals)
            .with_context(&context)
            .with_published_keys(&published);
        match iam::resource_decision(policy, &request) {
            Decision::Denied => return false,
            Decision::Allowed => granted = true,
            Decision::NotStated => {}
        }
    }
    granted
}

/// The KMS client the action's key is read through: the AWS SDK driven over the emulator's own
/// in-process transport, signing in the emulator's own name. Nothing here opens a socket or
/// leaves the host — `ctx.callback()` is the framework's in-process channel into this same
/// emulator's services — and the only caller-supplied value that reaches the client's
/// authority is the signing Region, which is a Region LABEL by the time it gets here
/// ([`crate::state::region_from_request`]), so no text of a request can steer the address.
fn kms_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The credentials the callback hands over are used directly rather than
        // cached.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
