// SPDX-License-Identifier: CC0-1.0

//! The resources a TRAIL REFERENCES, resolved in the services that own them.
//!
//! A trail is a delivery pipeline: "creates a trail that specifies the settings for
//! delivery of log data to an Amazon S3 bucket"
//! (`smithy_query(query="doc",shape="CreateTrail")`). Every resource its settings name
//! — the destination bucket, the notification topic, the encryption key and the
//! CloudWatch Logs destination — belongs to ANOTHER service, and CloudTrail resolves
//! each one before it accepts the trail, because a trail configured against a resource
//! that does not exist, or against one that does not let CloudTrail use it, could never
//! deliver a log file. Each failure has its own modelled error:
//! `S3BucketDoesNotExistException`, `InsufficientS3BucketPolicyException`,
//! `InsufficientSnsTopicPolicyException`, `KmsKeyNotFoundException`,
//! `InvalidKmsKeyIdException` and `InvalidCloudWatchLogsLogGroupArnException`.
//!
//! The resolutions live HERE, once, because both operations that configure a trail
//! (`CreateTrail` and `UpdateTrail`) apply the same rule to the same members: neither
//! can disagree with the other about which destinations, topics, keys or log groups a
//! trail may be pointed at.
//!
//! Whether a referenced resource LETS CloudTrail use it is the ordinary IAM question —
//! may this principal perform this action on this resource? — so it is answered by the
//! shared evaluation of resource policies ([`service_common::iam_authorization`]) over
//! the document the owning service holds, as the service principal
//! [`SERVICE_PRINCIPAL`], carrying the condition keys such a grant is scoped by. Nothing
//! here reads a policy by itself.
//!
//! LOCK ORDER: every function takes the OWNING service's store lock, holds no other
//! while it does and hands back an owned value, so each must be called with NO guard of
//! this service's own store held. That keeps the cross-service access one-directional
//! and a lock cycle between two stores unreachable.

use service_common::iam_authorization::{self, Decision, Request};

/// The service principal CloudTrail delivers a trail's log files and notifications as —
/// the principal a destination bucket's policy and a notification topic's policy name to
/// grant it.
pub const SERVICE_PRINCIPAL: &str = "cloudtrail.amazonaws.com";

/// The Amazon S3 action a log-file delivery performs on the destination bucket.
const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The Amazon SNS action a log-file notification performs on the topic.
const PUBLISH_ACTION: &str = "sns:Publish";

/// The condition key naming the canned ACL a request asks for, which a CloudTrail bucket
/// policy scopes its grant by.
const X_AMZ_ACL_CONDITION_KEY: &str = "s3:x-amz-acl";

/// The canned ACL CloudTrail writes a log file under, so the bucket's owner owns the
/// object it delivers.
const BUCKET_OWNER_FULL_CONTROL: &str = "bucket-owner-full-control";

/// The global condition key naming the resource a service-principal call is made ON
/// BEHALF OF: the trail, for a delivery CloudTrail makes for one.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The global condition key naming the account that resource belongs to.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// The condition keys a delivery CloudTrail makes FOR the trail `trail_arn` carries, so
/// a statement scoped by them is decided rather than left undecided.
///
/// These two and no others: they are the keys a grant to a SERVICE PRINCIPAL is scoped
/// by, naming the resource the call is made on behalf of and the account that resource
/// belongs to. A key about the CALLING account is deliberately not among them — the call
/// is not made by the trail's owner — so a statement conditioned on one stays undecided,
/// which is the fail-closed reading, and never turns into a grant for this delivery.
fn delivery_context(trail_arn: &str) -> Vec<(String, String)> {
    vec![
        (SOURCE_ARN_CONDITION_KEY.to_string(), trail_arn.to_string()),
        (
            SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ]
}

/// The CloudTrail path a trail's log files are delivered under, below the `S3KeyPrefix`
/// the trail carries: `AWSLogs/<account>/CloudTrail/<region>/`.
///
/// It is the resource a bucket policy's `s3:PutObject` grant is written over, so the
/// permission question is asked about the keys this trail would really write rather than
/// about the bucket as a whole — which is what makes a policy that grants the write
/// somewhere else insufficient for this trail. The layout itself belongs to the delivery
/// that writes those keys ([`service_common::trail_log_files`]), so it is read from there
/// rather than written again here.
fn log_file_key_prefix(s3_key_prefix: Option<&str>, region: &str) -> String {
    ::service_common::trail_log_files::log_file_key_prefix(
        s3_key_prefix,
        ::engine::emulator_account_id(),
        region,
    )
}

/// What the account's object store says about a trail's log-delivery destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationVerdict {
    /// The bucket exists and its policy lets CloudTrail write this trail's log files.
    ///
    /// It is also the answer when the object store cannot be reached at all — an engine
    /// this emulator's object store is not mounted on — which is a fact about the engine
    /// and not about the bucket, so a caller is served rather than refused on a
    /// resolution that never happened.
    Accepted,
    /// The account holds no bucket of that name, so the trail has nowhere to deliver to.
    NoSuchBucket,
    /// The bucket exists, and its policy does not let CloudTrail write this trail's log
    /// files into it.
    PolicyInsufficient,
}

/// Resolve a trail's log-delivery destination: the bucket has to exist, and its policy
/// has to let CloudTrail write the log files this trail would deliver.
///
/// `s3_key_prefix` is the prefix the trail delivers under and `region` its home Region,
/// because together they decide WHICH keys the delivery writes
/// ([`log_file_key_prefix`]); `trail_arn` is the trail the delivery is made for, which a
/// grant to the CloudTrail service principal is scoped by.
pub fn destination_verdict<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    s3_key_prefix: Option<&str>,
    region: &str,
    trail_arn: &str,
) -> DestinationVerdict {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return DestinationVerdict::Accepted;
    };
    // The object store's lock is taken, read and released before anything is decided, so
    // no other lock is ever held across it.
    let held = {
        let store = state.lock();
        match store.bucket(bucket) {
            None => return DestinationVerdict::NoSuchBucket,
            Some(held) => held.policy.clone(),
        }
    };
    // "A bucket is created WITHOUT a policy": a bucket that carries none grants the
    // CloudTrail service principal nothing, which is exactly the insufficient policy the
    // modelled error is about.
    let Some(policy) = held else {
        return DestinationVerdict::PolicyInsufficient;
    };
    let resource = format!(
        "{}/{}",
        ::service_com_amazonaws_s3_amazons3::state::bucket_arn(bucket),
        log_file_key_prefix(s3_key_prefix, region)
    );
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let mut context = delivery_context(trail_arn);
    context.push((
        X_AMZ_ACL_CONDITION_KEY.to_string(),
        BUCKET_OWNER_FULL_CONTROL.to_string(),
    ));
    let request =
        Request::new(PUT_OBJECT_ACTION, &resource, &principals).with_context(&context);
    match iam_authorization::resource_decision(&policy, &request) {
        Decision::Allowed => DestinationVerdict::Accepted,
        // A policy that states no grant for the delivery is as insufficient as one that
        // denies it: either way CloudTrail may not write the log file.
        Decision::Denied | Decision::NotStated => DestinationVerdict::PolicyInsufficient,
    }
}

/// What the account's Amazon SNS store says about a trail's notification topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicVerdict {
    /// The topic's access policy lets CloudTrail publish to it — or the Amazon SNS store
    /// could not be reached, on the terms [`DestinationVerdict::Accepted`] states.
    Accepted,
    /// The topic's access policy does not let CloudTrail publish to it, which is also
    /// the answer for a topic that does not exist: a topic the account does not hold
    /// carries no policy that could permit the publish, and the API models no
    /// topic-does-not-exist error of its own for this member.
    PolicyInsufficient,
}

/// Resolve a trail's notification topic: its access policy has to let CloudTrail publish
/// the log-file-delivered notification the trail asks for.
pub fn notification_topic_verdict<Eng: Send + Sync + 'static>(
    engine: &Eng,
    topic_arn: &str,
    trail_arn: &str,
) -> TopicVerdict {
    let Some(state) = ::service_com_amazonaws_sns_amazonsimplenotificationservice::state::
        service_state(engine)
    else {
        return TopicVerdict::Accepted;
    };
    // The topic reports the policy its owner set, or the one Amazon SNS generates for a
    // topic with none, exactly as `GetTopicAttributes` reports it.
    let policy = {
        let store = state.lock();
        store
            .topics
            .get(topic_arn)
            .map(|topic| topic.policy_attribute())
    };
    let Some(policy) = policy else {
        return TopicVerdict::PolicyInsufficient;
    };
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let context = delivery_context(trail_arn);
    let request = Request::new(PUBLISH_ACTION, topic_arn, &principals).with_context(&context);
    match iam_authorization::resource_decision(&policy, &request) {
        Decision::Allowed => TopicVerdict::Accepted,
        Decision::Denied | Decision::NotStated => TopicVerdict::PolicyInsufficient,
    }
}

/// What the account's KMS store says about a trail's `KmsKeyId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionKeyVerdict {
    /// The identifier names a key this account and Region holds — or the KMS store could
    /// not be reached, on the terms [`DestinationVerdict::Accepted`] states.
    Accepted,
    /// The identifier is well formed but names no key here.
    NotFound,
    /// The identifier is an ARN that is not a well-formed KMS key or alias ARN, so it
    /// names no key anywhere.
    MalformedArn,
}

/// Resolve a trail's `KmsKeyId`, in every form the member documents: "an alias name
/// prefixed by `alias/`, a fully specified ARN to an alias, a fully specified ARN to a
/// key, or a globally unique identifier"
/// (`smithy_query(query="doc",shape="CreateTrailRequest",member="KmsKeyId")`). The forms
/// differ only in how they SPELL one key, so the one resolver KMS itself uses reads all
/// of them.
///
/// `region` is the Region the request was signed for: a KMS key is a regional resource,
/// and the modelled `KmsKeyNotFoundException` covers a key that "does not exist" and one
/// that is "not in the same Region".
pub fn encryption_key_verdict<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key_id: &str,
) -> EncryptionKeyVerdict {
    let Some(state) = ::service_com_amazonaws_kms_trentservice::state::service_state(engine)
    else {
        return EncryptionKeyVerdict::Accepted;
    };
    let store = state.lock();
    match store.resolve(region, key_id) {
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::Found(_) => {
            EncryptionKeyVerdict::Accepted
        }
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::NotFound => {
            EncryptionKeyVerdict::NotFound
        }
        ::service_com_amazonaws_kms_trentservice::state::KeyRef::MalformedArn
        | ::service_com_amazonaws_kms_trentservice::state::KeyRef::UnauthorizedAccount => {
            EncryptionKeyVerdict::MalformedArn
        }
    }
}

/// Whether `arn` is a CloudWatch Logs log-group ARN naming a log group this account and
/// Region holds.
///
/// `CloudWatchLogsLogGroupArn` is documented as "a log group name using an Amazon
/// Resource Name (ARN) … You must use a log group that exists in your account"
/// (`smithy_query(query="doc",shape="CreateTrailRequest",member="CloudWatchLogsLogGroupArn")`),
/// so a value that is not such an ARN and one that names no group are the same defect:
/// the modelled `InvalidCloudWatchLogsLogGroupArnException`, "thrown when the provided
/// CloudWatch Logs log group is not valid".
pub fn log_group_is_valid<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    arn: &str,
) -> bool {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return false;
    };
    if parsed.vendor != "logs"
        || parsed.region != region
        || parsed.account != ::engine::emulator_account_id()
        || parsed.resource_type() != "log-group"
    {
        return false;
    }
    let name = ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::
        log_group_name_from_identifier(arn);
    if !::service_common::cloudwatch_logs::is_log_group_name(name) {
        return false;
    }
    let Some(state) =
        ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::service_state(engine)
    else {
        return true;
    };
    let store = state.lock();
    store.log_groups.contains_key(name)
}

/// What the object store holds of a trail's DELIVERED log files: the instant the newest one
/// under this trail's own log-file area was written, or `None` for a trail that has
/// delivered none.
///
/// It is read in the store that OWNS those objects rather than tracked here, so a trail
/// reports a delivery exactly when the bucket really holds the file it delivered: the one
/// account of what was delivered is the delivered object itself. The area read is the
/// trail's whole log-file prefix ([`service_common::trail_log_files::trail_log_file_prefix`])
/// — every Region the trail logs delivers under it — and only the objects a log file's name
/// ends with are counted, so an unrelated object a client put under the same prefix is not
/// read as a delivery.
pub fn latest_log_file_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    s3_key_prefix: Option<&str>,
) -> Option<::std::time::SystemTime> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let prefix = ::service_common::trail_log_files::trail_log_file_prefix(
        s3_key_prefix,
        ::engine::emulator_account_id(),
    );
    let store = state.lock();
    store
        .bucket(bucket)?
        .objects()
        .filter(|(key, _)| {
            key.starts_with(&prefix)
                && key.ends_with(::service_common::trail_log_files::LOG_FILE_SUFFIX)
        })
        .map(|(_, object)| object.last_modified)
        .max()
}

/// The refusal text for `InsufficientS3BucketPolicyException`: the destination bucket
/// whose policy does not let CloudTrail write the trail's log files.
///
/// Request-derived — the `S3BucketName` the caller sent — because the caller fixes the
/// call by putting a bucket policy on THAT bucket.
pub fn insufficient_s3_bucket_policy_message(bucket: &str) -> String {
    format!("Incorrect S3 bucket policy is detected for bucket: {bucket}")
}

/// The refusal text for `InsufficientSnsTopicPolicyException`: the notification topic
/// whose access policy does not let CloudTrail publish to it.
///
/// No recorded refusal pins this sentence, so it states the same rule the bucket's does
/// ([`insufficient_s3_bucket_policy_message`]) about the resource it was given — the
/// topic, named by the ARN the request's `SnsTopicName` resolved to, which is what the
/// caller changes the policy of.
pub fn insufficient_sns_topic_policy_message(topic_arn: &str) -> String {
    format!("Incorrect SNS topic policy is detected for topic: {topic_arn}")
}

/// The refusal text for `KmsKeyNotFoundException`: the `KmsKeyId` the request named,
/// inside the rule the model states for that error ("thrown when the KMS key does not
/// exist, when the S3 bucket and the KMS key are not in the same Region, or when the KMS
/// key associated with the Amazon SNS topic either does not exist or is not in the same
/// Region").
pub fn kms_key_not_found_message(key_id: &str) -> String {
    format!("KMS key {key_id} does not exist, or is not in the same Region as the trail")
}

/// The refusal text for `InvalidKmsKeyIdException`, "thrown when the KMS key ARN is not
/// valid": the value the request gave, which names no key anywhere.
pub fn invalid_kms_key_id_message(key_id: &str) -> String {
    format!("KMS key identifier {key_id} is not a valid key id, key ARN, alias name or alias ARN")
}

/// The refusal text for `InvalidCloudWatchLogsLogGroupArnException`, "thrown when the
/// provided CloudWatch Logs log group is not valid": the ARN the request gave, which
/// either is not a log-group ARN of this account and Region or names no log group.
pub fn invalid_log_group_arn_message(arn: &str) -> String {
    format!(
        "CloudWatch Logs log group ARN {arn} is not valid: it must name a log group that \
         exists in your account"
    )
}
