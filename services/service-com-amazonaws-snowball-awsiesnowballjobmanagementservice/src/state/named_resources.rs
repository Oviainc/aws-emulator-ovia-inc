// SPDX-License-Identifier: CC0-1.0

//! The AWS resources a Snow order NAMES, resolved against the services that hold them.
//!
//! An order is placed AGAINST resources that already exist: the Amazon S3 buckets the
//! device transfers and the Lambda functions and Amazon EC2 images it carries
//! (`JobResource`), the IAM role the transfer is performed under (`RoleARN`), the Key
//! Management Service key the data is encrypted with (`KmsKeyARN`) and the Amazon SNS
//! topics the order's notifications are published to (`Notification`). "Your Amazon Web
//! Services account must have the right trust policies and permissions in place to create
//! a job for a Snow device" (`CreateJob`), and the three refusals every order operation
//! declares for a resource it cannot use — `InvalidResourceException` ("The specified
//! resource can't be found. Check the information you provided in your last request, and
//! try again"), `KMSRequestFailedException` and `Ec2RequestFailedException`
//! (`smithy_query(query="operation", operation="CreateJob")`) — are what that resolution
//! answers with. So every one of those members is resolved BEFORE the order is written,
//! rather than stored as an order against resources nobody holds.
//!
//! Each value is resolved by ASKING THE SERVICE THAT OWNS IT, through the in-process
//! callback ([`smithy_server_runtime::OperationContext::callback`]) every emulated service
//! reaches another through: the AWS SDK driven over this emulator's own transport, so the
//! buckets, roles, keys, images, functions and topics read here are the very records those
//! services' own creates wrote, and this crate names no other service's crate. The read is
//! each owning service's own "is it there" call — `HeadBucket`, `GetRole`, `DescribeKey`,
//! `DescribeImages`, `GetFunction`, `GetTopicAttributes` — rather than a second inventory
//! kept here, which nothing would write.
//!
//! Nothing here opens a connection: the clients below are given the callback's own
//! connector, which hands the request back to this emulator's router, and the endpoint URL
//! each is built with is only how an SDK client addresses a request — the router then
//! routes it by its signing scope. No destination comes from the order's own values, and
//! the caller's own `Authorization` header is never carried over: the callback signs with
//! the emulator's own credentials.
//!
//! An order is refused ONLY on a service's own negative answer: the lookup reports
//! [`Resolution::DoesNotHold`] when the owning service answered, with a client status, one
//! of the error codes it uses for an identifier it holds no resource for. Everything else
//! says nothing about the resource — a deployment that mounts Snowball without that service,
//! a server fault, a reply carrying no service error code at all, a value this emulator
//! cannot read as naming a resource of this account — and leaves the order to go ahead, so
//! no order is refused on a condition nobody attributed to the resource it names.

use crate::ops::types::{
    Ec2RequestFailedException, InvalidResourceException, JobResource, KMSRequestFailedException,
    Notification,
};

/// The most resources one request resolves.
///
/// `JobResource`'s three lists are unbounded in the model, so the number of lookups one
/// request could otherwise ask for is the caller's to choose: the cap is what keeps the
/// in-process fan-out of a single order bounded. It is well above any real order — a Snow
/// device holds ten AMIs ("Currently, a Snowball Edge device can store 10 AMIs",
/// `ListCompatibleImages`) and an order names a handful of buckets — so a request within
/// it has every resource it names resolved; past it the remaining values are left
/// unresolved rather than refused, exactly as a value no service answered about is.
pub const MAX_RESOLVED_NAMED_RESOURCES: usize = 32;

/// The resources an order names, as the operation that took the request reads them.
///
/// One struct rather than six arguments, because all four operations that place or update
/// an order (`CreateJob`, `UpdateJob`, `CreateCluster`, `UpdateCluster`) name the same
/// resources through the same members — the two that model no `KmsKeyARN` simply leave that
/// field `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NamedResources<'a> {
    /// `RoleARN`: "The RoleARN that you want to associate with this job".
    pub role_arn: Option<&'a str>,
    /// `KmsKeyARN`: the key the order's data is encrypted under, on the operations that
    /// model it.
    pub kms_key_arn: Option<&'a str>,
    /// `Resources`: the buckets, functions and images the order covers.
    pub resources: Option<&'a JobResource>,
    /// `Notification`: the topics the order's state changes are published to.
    pub notification: Option<&'a Notification>,
}

/// The modelled refusal a resource the owning service does not hold earns, as the shape it
/// travels in.
///
/// The three shapes are shared by every order operation, while each operation's error ENUM
/// is its own, so this names the shape and the handler lifts it into its own channel.
#[derive(Debug)]
pub enum NamedResourceRefusal {
    /// A bucket, role, function or topic the owning service holds no record of: "The
    /// specified resource can't be found" is what this operation answers for a resource it
    /// cannot resolve.
    InvalidResource(InvalidResourceException),
    /// A key Key Management Service answered it cannot use for this order.
    KmsRequestFailed(KMSRequestFailedException),
    /// An image Amazon EC2 answered it holds no record of.
    Ec2RequestFailed(Ec2RequestFailedException),
}

/// The first resource this order names that the service owning it answered it does not
/// hold, as the refusal that resource's kind earns — or `None` when every resource named
/// resolved, or nothing could be resolved at all.
///
/// `operation` is the operation the request was made to, which the Key Management Service
/// refusal names ("the specified CreateJob or UpdateJob action").
///
/// Awaited BEFORE this service's own store is locked, so no lock is held across the
/// in-process calls this makes.
pub async fn first_unresolvable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    named: NamedResources<'_>,
) -> Option<NamedResourceRefusal> {
    let region = crate::state::request_region(ctx.parts());
    for (kind, value) in named_values(named) {
        if resolve(ctx, &region, kind, value).await == Resolution::DoesNotHold {
            return Some(kind.refusal(operation, value));
        }
    }
    None
}

/// A resource an order names, which is to say the service that holds it and the member that
/// named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    /// `S3Resource$BucketArn`: "The Amazon Resource Name (ARN) of an Amazon S3 bucket".
    S3Bucket,
    /// `LambdaResource$LambdaArn`: "An Amazon Resource Name (ARN) that represents an Lambda
    /// function".
    LambdaFunction,
    /// `Ec2AmiResource$AmiId`: "The ID of the AMI in Amazon EC2".
    Ec2Image,
    /// `RoleARN`, an IAM role of this account.
    IamRole,
    /// `KmsKeyARN`, a Key Management Service key.
    KmsKey,
    /// `Notification$SnsTopicARN` and `Notification$DevicePickupSnsTopicARN`.
    SnsTopic,
}

impl ResourceKind {
    /// The refusal a resource of this kind the owning service does not hold earns.
    ///
    /// Amazon EC2 and Key Management Service have a refusal of their own on every order
    /// operation, so an image and a key are answered with theirs; every other resource is
    /// answered with the one refusal the model declares for a resource that cannot be
    /// resolved.
    fn refusal(self, operation: &str, value: &str) -> NamedResourceRefusal {
        match self {
            // "Your user lacks the necessary Amazon EC2 permissions to perform the
            // attempted action" (`Ec2RequestFailedException`), which is what this
            // operation declares for an Amazon EC2 request it could not carry out.
            Self::Ec2Image => NamedResourceRefusal::Ec2RequestFailed(Ec2RequestFailedException {
                message: Some(
                    "Your user lacks the necessary Amazon EC2 permissions to perform the \
                     attempted action."
                        .to_string(),
                ),
            }),
            // "The provided Key Management Service key lacks the permissions to perform
            // the specified CreateJob or UpdateJob action" (`KMSRequestFailedException`):
            // the action is the one the request was made to.
            Self::KmsKey => NamedResourceRefusal::KmsRequestFailed(KMSRequestFailedException {
                message: Some(format!(
                    "The provided Key Management Service key lacks the permissions to perform \
                     the specified {operation} action."
                )),
            }),
            // "The specified resource can't be found. Check the information you provided in
            // your last request, and try again" (`InvalidResourceException`), worded over
            // the resource at fault the way every other refusal of this operation words it
            // (the address, the cluster, the long-term pricing contract), and carrying the
            // kind of resource in the member the model has for it: `ResourceType`, "the
            // provided resource value is invalid".
            _ => NamedResourceRefusal::InvalidResource(InvalidResourceException {
                message: Some(format!(
                    "The {} {value} can't be found. Check the information you provided in your \
                     last request, and try again.",
                    self.label()
                )),
                resource_type: Some(self.resource_type().to_string()),
            }),
        }
    }

    /// How a refusal names a resource of this kind, as the service it belongs to is called.
    fn label(self) -> &'static str {
        match self {
            Self::S3Bucket => "Amazon S3 bucket",
            Self::LambdaFunction => "Lambda function",
            Self::Ec2Image => "Amazon EC2 AMI",
            Self::IamRole => "IAM role",
            Self::KmsKey => "Key Management Service key",
            Self::SnsTopic => "Amazon SNS topic",
        }
    }

    /// The `InvalidResourceException$ResourceType` a resource of this kind is reported
    /// under, in the camel-case form this service's other refusals use (`address`,
    /// `cluster`, `longTermPricing`, `job`).
    fn resource_type(self) -> &'static str {
        match self {
            Self::S3Bucket => "s3Bucket",
            Self::LambdaFunction => "lambdaFunction",
            Self::Ec2Image => "ec2Ami",
            Self::IamRole => "role",
            Self::KmsKey => "kmsKey",
            Self::SnsTopic => "snsTopic",
        }
    }
}

/// What the service owning a resource answered about the value an order named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// The service reported the resource.
    Holds,
    /// The service answered that it holds no resource that value names.
    DoesNotHold,
    /// Nothing was learned about the value: see this module's header.
    Unknown,
}

/// Every resource this order names, each once, in a fixed order so a request naming more
/// than one unresolvable resource is answered the same way every time.
///
/// A member supplied EMPTY names nothing — `RoleARN`, `KmsKeyARN`, `S3Resource$BucketArn`,
/// `LambdaResource$LambdaArn` and the two topic members are each modelled `length 0..`, so
/// the empty string is a value they admit, and this service reads a member left out and a
/// member sent empty the same way — so it is not looked up. Repeats are dropped: an order
/// that names one bucket in several `S3Resource` entries asks one question about it, and the
/// number of lookups is capped at [`MAX_RESOLVED_NAMED_RESOURCES`] whatever the request
/// sends.
fn named_values(named: NamedResources<'_>) -> Vec<(ResourceKind, &str)> {
    let resources = named.resources;
    let s3_buckets = resources
        .and_then(|resources| resources.s3_resources.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|resource| (ResourceKind::S3Bucket, resource.bucket_arn.as_deref()));
    let lambda_functions = resources
        .and_then(|resources| resources.lambda_resources.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|resource| (ResourceKind::LambdaFunction, resource.lambda_arn.as_deref()));
    let ec2_images = resources
        .and_then(|resources| resources.ec2_ami_resources.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|resource| (ResourceKind::Ec2Image, Some(resource.ami_id.as_str())));
    let topics = [
        named
            .notification
            .and_then(|notification| notification.sns_topic_arn.as_deref()),
        named
            .notification
            .and_then(|notification| notification.device_pickup_sns_topic_arn.as_deref()),
    ]
    .into_iter()
    .map(|topic| (ResourceKind::SnsTopic, topic));

    let mut seen: Vec<(ResourceKind, &str)> = Vec::new();
    for (kind, value) in [
        (ResourceKind::IamRole, named.role_arn),
        (ResourceKind::KmsKey, named.kms_key_arn),
    ]
    .into_iter()
    .chain(s3_buckets)
    .chain(lambda_functions)
    .chain(ec2_images)
    .chain(topics)
    {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            continue;
        };
        if seen.contains(&(kind, value)) {
            continue;
        }
        if seen.len() == MAX_RESOLVED_NAMED_RESOURCES {
            ::tracing::debug!(
                resolved = MAX_RESOLVED_NAMED_RESOURCES,
                "this order names more resources than one request resolves; the rest are \
                 left unresolved"
            );
            break;
        }
        seen.push((kind, value));
    }
    seen
}

/// Ask the service that owns a resource of `kind` about `value`.
async fn resolve(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: ResourceKind,
    value: &str,
) -> Resolution {
    match kind {
        ResourceKind::S3Bucket => {
            // A bucket ARN names its bucket in the resource field (`arn:aws:s3:::bucket`),
            // read through the crate's ONE caller-supplied-ARN parse. The member's own
            // modelled pattern (`^arn:aws.*:*$`) admits values that name no S3 bucket at
            // all, and there is no bucket for S3 to be asked about in one of those.
            let Some(bucket) = bucket_name(value) else {
                return unreadable(kind, value);
            };
            match s3_client(ctx, region).head_bucket().bucket(bucket).send().await {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, S3_ABSENT_CODES, kind, value),
            }
        }
        ResourceKind::LambdaFunction => {
            match lambda_client(ctx, region)
                .get_function()
                .function_name(value)
                .send()
                .await
            {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, LAMBDA_ABSENT_CODES, kind, value),
            }
        }
        ResourceKind::Ec2Image => {
            match ec2_client(ctx, region)
                .describe_images()
                .image_ids(value)
                .send()
                .await
            {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, EC2_ABSENT_CODES, kind, value),
            }
        }
        ResourceKind::IamRole => {
            // An IAM role ARN names the role IAM holds it under, read through the one
            // reading every service resolving a caller-supplied role shares. It answers
            // nothing for an ARN of ANOTHER account, whose roles are not this emulator's
            // IAM to be asked about.
            let Some(role_name) = ::service_common::iam_role::role_name(value) else {
                return unreadable(kind, value);
            };
            // The IAM client is the ONE construction every service resolving a
            // caller-supplied IAM entity shares (`service_common::iam_client`), rather than
            // a seventh copy of the same configuration here.
            match ::service_common::iam_client::iam_client(ctx, region)
                .get_role()
                .role_name(role_name)
                .send()
                .await
            {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, IAM_ABSENT_CODES, kind, value),
            }
        }
        ResourceKind::KmsKey => {
            // `DescribeKey` resolves every form of identifier KMS accepts — a key id, a
            // key ARN, an alias name or an alias ARN — so nothing here inspects the value
            // itself.
            match kms_client(ctx, region)
                .describe_key()
                .key_id(value)
                .send()
                .await
            {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, KMS_ABSENT_CODES, kind, value),
            }
        }
        ResourceKind::SnsTopic => {
            match sns_client(ctx, region)
                .get_topic_attributes()
                .topic_arn(value)
                .send()
                .await
            {
                Ok(_) => Resolution::Holds,
                Err(error) => answered_absent(&error, SNS_ABSENT_CODES, kind, value),
            }
        }
    }
}

/// The bucket an `S3Resource$BucketArn` names, or `None` when the value does not name an
/// Amazon S3 bucket: an S3 object ARN opens its resource field with the bucket
/// (`arn:aws:s3:::bucket/key`), so the bucket is the resource field up to its first `/`.
fn bucket_name(bucket_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(bucket_arn, S3_ARN_VENDOR)?;
    let bucket = arn.resource.split('/').next()?;
    (!bucket.is_empty()).then_some(bucket)
}

/// The vendor field of an Amazon S3 ARN, the service namespace S3 publishes.
const S3_ARN_VENDOR: &str = "s3";

/// Whether the error a lookup came back with is the owning service ANSWERING that it holds
/// no resource the value names: a client status, carrying one of the codes that service
/// uses for an identifier it cannot resolve.
///
/// Anything else is [`Resolution::Unknown`] — a 5xx is about the service rather than about
/// the resource, whatever code travels with it, and a reply carrying no code of that
/// service at all is a routing failure rather than an answer, as when the service is not
/// mounted beside this one.
fn answered_absent<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    absent_codes: &[&str],
    kind: ResourceKind,
    value: &str,
) -> Resolution
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
{
    let code = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error)
        .unwrap_or_default()
        .to_string();
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    if client_error && absent_codes.iter().any(|known| known.eq_ignore_ascii_case(&code)) {
        return Resolution::DoesNotHold;
    }
    ::tracing::debug!(
        resource = value,
        resource_kind = ?kind,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "the service that owns this resource did not answer whether the account holds it; \
         the order is not refused on it"
    );
    Resolution::Unknown
}

/// A value this emulator cannot read as naming a resource of this account at all: there is
/// nothing for the owning service to be asked, so nothing is known about it.
fn unreadable(kind: ResourceKind, value: &str) -> Resolution {
    ::tracing::debug!(
        resource = value,
        resource_kind = ?kind,
        "this value names no resource of this account that the owning service could be \
         asked about; the order is not refused on it"
    );
    Resolution::Unknown
}

/// The Amazon S3 error codes that say the account holds no such bucket: the code a HEAD of
/// an absent bucket answers ("the HEAD request returns a generic 400 Bad Request, 403
/// Forbidden, or 404 Not Found HTTP status code", `HeadBucket`) and the one S3 names a
/// bucket it does not hold with elsewhere.
const S3_ABSENT_CODES: &[&str] = &["NotFound", "NoSuchBucket"];

/// The IAM error code for an entity the account does not hold: "The request was rejected
/// because it referenced a resource entity that does not exist" (`NoSuchEntityException`,
/// whose query error code is `NoSuchEntity`).
const IAM_ABSENT_CODES: &[&str] = &["NoSuchEntity"];

/// The Key Management Service codes that say the identifier names no key KMS can use: the
/// two `DescribeKey` models for an identifier it cannot resolve.
///
/// Deliberately NOT the state codes (`DisabledException`, `KMSInvalidStateException`): a
/// key in one of those states does resolve, and the order only uses it later.
const KMS_ABSENT_CODES: &[&str] = &["NotFoundException", "InvalidArnException"];

/// The Amazon EC2 codes that say the account can see no image of that id.
const EC2_ABSENT_CODES: &[&str] = &["InvalidAMIID.NotFound", "InvalidAMIID.Malformed"];

/// The Lambda code for a function the account does not hold: "The resource specified in the
/// request does not exist" (`ResourceNotFoundException`).
const LAMBDA_ABSENT_CODES: &[&str] = &["ResourceNotFoundException"];

/// The Amazon SNS code for a topic that does not exist: "Indicates that the requested
/// resource does not exist" (`NotFoundException`, whose query error code is `NotFound`).
const SNS_ABSENT_CODES: &[&str] = &["NotFound"];

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so a resource is resolved where the order
/// that names it is placed. The in-process transport routes on the request's own path, and
/// a virtual-host bucket name is not a host this emulator resolves, so the bucket is
/// addressed in the path. There is nothing to retry, time out or watch for a stalled stream
/// over that transport — each would need a sleep implementation this client is not given —
/// and the credentials the callback hands over are used directly.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The client each remaining service is asked through, configured exactly as [`s3_client`]
/// is and for the same reasons. IAM is not among them: the one construction of that client
/// already lives in `service_common::iam_client`, which this module calls.
///
/// One macro rather than four copies of the same builder: each SDK names its own `Config`
/// type, so the builders cannot be one function, but they are one piece of configuration
/// and a change to it has to reach all of them.
macro_rules! in_process_client {
    ($name:ident, $sdk:ident, $endpoint_prefix:literal) => {
        fn $name(
            ctx: &smithy_server_runtime::OperationContext<'_>,
            region: &str,
        ) -> ::$sdk::Client {
            let config = ::$sdk::Config::builder()
                .behavior_version(::$sdk::config::BehaviorVersion::latest())
                .region(::$sdk::config::Region::new(region.to_string()))
                .endpoint_url(format!(
                    "https://{}.{region}.amazonaws.com",
                    $endpoint_prefix
                ))
                .credentials_provider(ctx.callback().aws_credentials_provider())
                .http_client(ctx.callback().http_client())
                .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::$sdk::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::$sdk::config::StalledStreamProtectionConfig::disabled(),
                )
                .build();
            ::$sdk::Client::from_conf(config)
        }
    };
}

in_process_client!(kms_client, aws_sdk_kms, "kms");
in_process_client!(ec2_client, aws_sdk_ec2, "ec2");
in_process_client!(lambda_client, aws_sdk_lambda, "lambda");
in_process_client!(sns_client, aws_sdk_sns, "sns");

#[cfg(test)]
mod tests {
    use super::{bucket_name, named_values, NamedResources, ResourceKind};
    use crate::ops::types::{
        Ec2AmiResource, JobResource, LambdaResource, Notification, S3Resource,
    };

    /// The bucket is read from the resource field of an Amazon S3 ARN, and from nothing
    /// else.
    #[test]
    fn a_bucket_is_read_only_from_an_s3_arn() {
        // (case, value, the bucket the reading must report)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a bucket ARN, which carries no Region or account",
                "arn:aws:s3:::import-source",
                Some("import-source"),
            ),
            (
                "an object ARN, whose resource opens with the bucket",
                "arn:aws:s3:::import-source/prefix/key.txt",
                Some("import-source"),
            ),
            (
                "another partition",
                "arn:aws-cn:s3:::import-source",
                Some("import-source"),
            ),
            (
                "a bucket name holding every character S3 admits in one",
                "arn:aws:s3:::my.bucket-name.123",
                Some("my.bucket-name.123"),
            ),
            (
                "another vendor's ARN",
                "arn:aws:s3-outposts:::import-source",
                None,
            ),
            ("an ARN naming no resource", "arn:aws:s3:::", None),
            ("a bare bucket name", "import-source", None),
            ("the empty string", "", None),
        ];
        for (case, value, expected) in rows {
            let read = bucket_name(value);
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// Every member that names a resource is collected, each value once, with the empty
    /// values left out and the total capped.
    #[test]
    fn every_named_resource_is_collected_once() {
        let resources = JobResource {
            s3_resources: Some(vec![
                S3Resource {
                    bucket_arn: Some("arn:aws:s3:::import-source".to_string()),
                    ..S3Resource::default()
                },
                // The same bucket again, which asks one question.
                S3Resource {
                    bucket_arn: Some("arn:aws:s3:::import-source".to_string()),
                    ..S3Resource::default()
                },
                // A member supplied empty names nothing.
                S3Resource {
                    bucket_arn: Some(String::new()),
                    ..S3Resource::default()
                },
            ]),
            lambda_resources: Some(vec![LambdaResource {
                lambda_arn: Some("arn:aws:lambda:us-west-2:000000000000:function:f".to_string()),
                ..LambdaResource::default()
            }]),
            ec2_ami_resources: Some(vec![Ec2AmiResource {
                ami_id: "ami-00000000".to_string(),
                ..Ec2AmiResource::default()
            }]),
        };
        let notification = Notification {
            sns_topic_arn: Some("arn:aws:sns:us-west-2:000000000000:order".to_string()),
            ..Notification::default()
        };
        let named = NamedResources {
            role_arn: Some("arn:aws:iam::000000000000:role/order"),
            kms_key_arn: None,
            resources: Some(&resources),
            notification: Some(&notification),
        };
        let collected = named_values(named);
        let expected = vec![
            (ResourceKind::IamRole, "arn:aws:iam::000000000000:role/order"),
            (ResourceKind::S3Bucket, "arn:aws:s3:::import-source"),
            (
                ResourceKind::LambdaFunction,
                "arn:aws:lambda:us-west-2:000000000000:function:f",
            ),
            (ResourceKind::Ec2Image, "ami-00000000"),
            (
                ResourceKind::SnsTopic,
                "arn:aws:sns:us-west-2:000000000000:order",
            ),
        ];
        assert_eq!(
            collected, expected,
            "case \"an order naming one of each\": expected {expected:?}, got {collected:?}"
        );

        // An order naming more resources than one request resolves has the rest left
        // unresolved rather than refused.
        let buckets: Vec<S3Resource> = (0..super::MAX_RESOLVED_NAMED_RESOURCES + 10)
            .map(|index| S3Resource {
                bucket_arn: Some(format!("arn:aws:s3:::bucket-{index}")),
                ..S3Resource::default()
            })
            .collect();
        let many = JobResource {
            s3_resources: Some(buckets),
            ..JobResource::default()
        };
        let capped = named_values(NamedResources {
            resources: Some(&many),
            ..NamedResources::default()
        });
        assert_eq!(
            capped.len(),
            super::MAX_RESOLVED_NAMED_RESOURCES,
            "case \"an order naming more buckets than the cap\": expected {} lookups, got {}",
            super::MAX_RESOLVED_NAMED_RESOURCES,
            capped.len()
        );
    }
}
