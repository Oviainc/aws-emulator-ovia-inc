// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an Amazon S3 location a SageMaker request names, and the ONE question
//! a create asks about it before it stores anything: can the resource's execution role READ
//! what that location names?
//!
//! Several SageMaker creates name an Amazon S3 location their resource's work reads — a
//! model's artifact (`ContainerDefinition$ModelDataUrl`, `S3ModelDataSource$S3Uri`), a
//! transform job's input (`TransformS3DataSource$S3Uri`), a training job's channels
//! (`S3DataSource$S3Uri`) — and each of them has to settle the SAME question before it
//! accepts the request: the location is read AS the execution role, so a location that names
//! no object, or one the role is denied the read of, backs no work at all. That question is
//! stated here once, for every operation that asks it, rather than per handler.
//!
//! # Only what Amazon S3 and the documents ANSWERED refuses
//!
//! Amazon S3 is a resource of ANOTHER service, reached the way any emulated service reaches
//! another: the AWS SDK over the emulator's OWN in-process callback ([`s3_client`]). So the
//! answer is never guessed:
//!
//! * a listing that ANSWERED and holds no object under the location, and an answer that
//!   names the absence itself (no such bucket, no such key), is the location naming nothing;
//! * every other outcome — no Amazon S3 mounted to ask, a transport that failed, an answer
//!   this emulator could not read — teaches nothing about the location and refuses nothing;
//! * a bucket policy governs the read of its bucket's objects: it refuses what it
//!   EXPLICITLY DENIES — IAM's evaluation order gives an explicit `Deny` precedence over
//!   every `Allow`, including the identity-based grant the role carries ("Determining
//!   whether a request is allowed or denied within an account") — and grants only what it
//!   ALLOWS, which is why SageMaker's documentation tells you to grant the execution role or
//!   the SageMaker service principal access to an artifact's bucket in that policy. It is
//!   decided through the tree's one evaluation of a policy document
//!   ([`::service_common::iam_authorization`]). A bucket that carries no policy, or one this
//!   emulator cannot read, withholds nothing.
//!
//! Nothing here holds a store guard: every question is awaited.

use super::*;

/// The IAM action reading one object is authorized under: "you must have permission to
/// perform the `s3:GetObject` action" (`GetObject`'s own documentation).
pub const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The IAM action writing an object is authorized under: "you must have the `s3:PutObject`
/// permission" (`PutObject`'s own documentation).
pub const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The IAM action listing a location is authorized under: "you must have permission to
/// perform the `s3:ListBucket` action" (`ListObjectsV2`'s own documentation). It is granted
/// on the BUCKET rather than on an object, which is what a refusal names.
pub const LIST_BUCKET_ACTION: &str = "s3:ListBucket";

/// The partition an Amazon S3 ARN carries when the execution role's own ARN names none this
/// emulator can read.
pub const DEFAULT_PARTITION: &str = "aws";

/// An `s3://bucket/key` URI split into its parts.
///
/// The modelled `S3Uri` pattern (`^(https|s3)://([^/]+)/?(.*)$`) is enforced before a handler
/// runs, so a value reaching here carries a bucket; `None` for one this reading cannot take a
/// bucket off.
#[derive(Debug, Clone)]
pub struct S3Location {
    pub bucket: String,
    pub key: String,
}

impl S3Location {
    /// The location `uri` names, or `None` for a value that names no `s3://` bucket.
    #[must_use]
    pub fn parse(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix("s3://")?;
        let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
        (!bucket.is_empty()).then(|| Self {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    /// This location's URI, as the caller wrote one.
    #[must_use]
    pub fn uri(&self) -> String {
        format!("s3://{}/{}", self.bucket, self.key)
    }

    /// The ARN of the object this location names, which is the resource an Amazon S3 action
    /// on it is authorized against (`arn:<partition>:s3:::<bucket>/<key>` — an Amazon S3
    /// object ARN carries no Region and no account).
    #[must_use]
    pub fn object_arn(&self, partition: &str) -> String {
        format!("arn:{partition}:s3:::{}/{}", self.bucket, self.key)
    }

    /// The ARN of the BUCKET this location is in, which is the resource a listing of it is
    /// authorized against (`arn:<partition>:s3:::<bucket>`).
    #[must_use]
    pub fn bucket_arn(&self, partition: &str) -> String {
        format!("arn:{partition}:s3:::{}", self.bucket)
    }
}

/// The partition the ARN `arn` carries, or [`DEFAULT_PARTITION`] for a value this emulator
/// cannot read one out of.
#[must_use]
pub fn arn_partition(arn: &str) -> String {
    ::service_common::arn::parse(arn)
        .map_or(DEFAULT_PARTITION, |parsed| parsed.partition)
        .to_string()
}

/// Why one Amazon S3 location a request names cannot be read as the execution role, with the
/// location AS THE CALLER WROTE IT.
///
/// Two conditions, because a caller is told different things about them: an object that is
/// not there at all, and one a policy withholds the read of (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadableLocation<'uri> {
    /// Amazon S3 ANSWERED that the location holds no object.
    NoObject(&'uri str),
    /// A bucket policy that CAN be read WITHHOLDS the read from this principal.
    Denied(&'uri str),
}

impl<'uri> UnreadableLocation<'uri> {
    /// The location this is about, as the request wrote it.
    #[must_use]
    pub fn uri(self) -> &'uri str {
        match self {
            Self::NoObject(uri) | Self::Denied(uri) => uri,
        }
    }
}

/// The FIRST of `locations` this emulator's own Amazon S3 answers the execution role cannot
/// read, as the request wrote it, and WHY — and `None` when it answers that about none of
/// them.
///
/// This is the question a create asks about every Amazon S3 location its resource's work
/// would read, in the order the request carries them, so the refusal names the same location
/// whichever others the request also names. A value that names no `s3://` bucket is left to
/// the modelled `@pattern` on the member and asked nothing here.
///
/// "Unreadable" is the pair of facts stated in this module's docs: the location names no
/// object, or the bucket policy explicitly denies the role the read. Both are grounded in an
/// ANSWER; neither is guessed.
pub async fn first_unreadable_location<'uri>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    locations: impl IntoIterator<Item = &'uri str>,
) -> Option<UnreadableLocation<'uri>> {
    let partition = arn_partition(role_arn);
    let client = s3_client(ctx, region);
    let mut asked = ::std::collections::BTreeSet::new();
    for uri in locations {
        if !asked.insert(uri) {
            continue;
        }
        let Some(location) = S3Location::parse(uri) else {
            continue;
        };
        // The policy is asked first: where a bucket carries one, it is what governs the
        // read, and Amazon S3 answers nothing about an object it withholds.
        if bucket_policy_withholds_read(&client, &location, role_arn, &partition).await {
            return Some(UnreadableLocation::Denied(uri));
        }
        if location_holds_no_object(&client, &location).await {
            return Some(UnreadableLocation::NoObject(uri));
        }
    }
    None
}

/// Whether Amazon S3 ANSWERED that `location` holds no object: a listing under the location's
/// key that came back empty, or an answer naming the absence of the bucket or the key itself.
///
/// The key is read as a PREFIX, which is the one reading that serves both forms a location
/// takes: a key naming one object lists that object, and a key naming a prefix lists what is
/// under it. Only one object is asked for, because the question is whether there is any.
pub async fn location_holds_no_object(
    client: &::aws_sdk_s3::Client,
    location: &S3Location,
) -> bool {
    let listed = client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&location.key)
        .max_keys(1)
        .send()
        .await;
    match listed {
        Ok(listed) => listed.contents().is_empty(),
        Err(error) => {
            // An answer that NAMES the absence is an answer about the location; every other
            // failure teaches nothing about it (see the module docs).
            let absent = error.as_service_error().is_some_and(|service| {
                service.is_no_such_bucket()
                    || ::aws_sdk_s3::error::ProvideErrorMetadata::code(service)
                        == Some(NO_SUCH_KEY_CODE)
            });
            if !absent {
                ::tracing::debug!(
                    bucket = ?location.bucket,
                    key = ?location.key,
                    "the location could not be listed, so nothing is refused for it"
                );
            }
            absent
        }
    }
}

/// The error code Amazon S3 answers a read of an object it does not hold with
/// (`GetObject`'s `NoSuchKey`), which is the second answer that names the absence of
/// [`location_holds_no_object`]'s object.
const NO_SUCH_KEY_CODE: &str = "NoSuchKey";

/// Whether the policy on `location`'s bucket WITHHOLDS the read of the object `location`
/// names from the SageMaker read made under the role `role_arn`.
///
/// Where a bucket carries a policy, that policy is what governs the read of its objects: it
/// refuses what it explicitly DENIES — an explicit `Deny` wins over every `Allow`, the role's
/// own identity-based grant included — and grants only what it ALLOWS, which is why
/// SageMaker's own documentation tells you to grant the execution role (or the SageMaker
/// service principal) access to the artifact's bucket in that policy. A bucket that carries
/// NO policy, or one this emulator cannot read, withholds nothing.
///
/// Why a policy that merely states nothing for the role withholds the read too, where IAM's
/// own rule within ONE account would let the role's identity grant stand
/// ([`::service_common::iam_authorization::is_allowed`]): the artifact a model names is read
/// ACROSS accounts as often as inside one — it is the recorded condition this answer exists
/// for — and a resource-based grant is what a cross-account read needs. The alternative, a
/// policy whose silence is a grant, makes a fenced-off artifact indistinguishable from an open
/// one, which is the defect this reading is here to stop.
///
/// The decision is the tree's one evaluation of a policy document, against every spelling the
/// read's principal is named by: the execution role's own ARN, the account spellings a
/// statement over the whole account is written with
/// ([`::service_common::iam_authorization::account_principals`]), and the SageMaker service
/// principal — which is what a bucket policy about a SageMaker artifact names.
pub async fn bucket_policy_withholds_read(
    client: &::aws_sdk_s3::Client,
    location: &S3Location,
    role_arn: &str,
    partition: &str,
) -> bool {
    let answered = client
        .get_bucket_policy()
        .bucket(&location.bucket)
        .send()
        .await;
    let Ok(answered) = answered else {
        // No policy on the bucket, no bucket, or no Amazon S3 to ask: nothing withheld.
        return false;
    };
    let Some(document) = answered.policy() else {
        return false;
    };
    let mut principals = vec![role_arn.to_string(), SERVICE_PRINCIPAL.to_string()];
    principals.extend(::service_common::iam_authorization::account_principals(
        role_arn,
    ));
    let object_arn = location.object_arn(partition);
    let request = ::service_common::iam_authorization::Request::new(
        GET_OBJECT_ACTION,
        &object_arn,
        &principals,
    );
    ::service_common::iam_authorization::resource_decision(document, &request)
        != ::service_common::iam_authorization::Decision::Allowed
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's seam for calling another emulated service: it routes in
/// process, so the `endpoint_url` below is an ADDRESS the in-process router reads rather than
/// a host any resolver looks up, no socket is opened and this client reaches the emulator's
/// own Amazon S3 and nothing else. There is nothing to retry, time out or watch for a stalled
/// stream, each of which would need a sleep implementation this client is not given; the same
/// goes for caching the identity.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let region = ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
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

#[cfg(test)]
mod tests {
    use super::S3Location;

    /// RULE: a location is the bucket and the key an `s3://` URI names, and a value that
    /// names no bucket is no location at all.
    ///
    /// The rows differ along the key's shape: none, one segment, several, a trailing
    /// delimiter, and a value of another scheme.
    #[test]
    fn a_location_is_the_bucket_and_key_an_s3_uri_names() {
        /// One URI, and the bucket and key it names.
        type Row = (&'static str, &'static str, Option<(&'static str, &'static str)>);

        // (case, the URI, the bucket and key it names)
        let rows: [Row; 7] = [
            ("a bucket and an object", "s3://bucket/model.tar.gz", Some(("bucket", "model.tar.gz"))),
            (
                "a nested key",
                "s3://bucket/models/one/model.tar.gz",
                Some(("bucket", "models/one/model.tar.gz")),
            ),
            ("a prefix", "s3://bucket/models/", Some(("bucket", "models/"))),
            ("a bucket alone", "s3://bucket", Some(("bucket", ""))),
            ("a bucket and an empty key", "s3://bucket/", Some(("bucket", ""))),
            ("another scheme", "https://bucket.s3.amazonaws.com/key", None),
            ("no bucket at all", "s3:///key", None),
        ];
        for (case, uri, expected) in rows {
            let parsed = S3Location::parse(uri);
            let read = parsed
                .as_ref()
                .map(|location| (location.bucket.as_str(), location.key.as_str()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: an Amazon S3 ARN carries no Region and no account, and the partition is the
    /// one the role's own ARN names.
    #[test]
    fn an_object_arn_names_the_bucket_the_key_and_the_partition() {
        let location = S3Location::parse("s3://artifacts/models/model.tar.gz")
            .expect("the URI names a location");
        for (case, partition, object, bucket) in [
            (
                "the commercial partition",
                "aws",
                "arn:aws:s3:::artifacts/models/model.tar.gz",
                "arn:aws:s3:::artifacts",
            ),
            (
                "another partition",
                "aws-cn",
                "arn:aws-cn:s3:::artifacts/models/model.tar.gz",
                "arn:aws-cn:s3:::artifacts",
            ),
        ] {
            assert_eq!(
                location.object_arn(partition),
                object,
                "case {case:?}: expected {object:?}, got {:?}",
                location.object_arn(partition)
            );
            assert_eq!(
                location.bucket_arn(partition),
                bucket,
                "case {case:?}: expected {bucket:?}, got {:?}",
                location.bucket_arn(partition)
            );
        }
    }

    /// RULE: the partition a refusal and an ARN carry is the one the ROLE's ARN names, and
    /// the commercial partition for a value naming none.
    #[test]
    fn the_partition_comes_from_the_roles_own_arn() {
        for (case, role, partition) in [
            ("this partition", "arn:aws:iam::000000000000:role/exec", "aws"),
            ("another partition", "arn:aws-cn:iam::000000000000:role/exec", "aws-cn"),
            ("a value that is no ARN", "exec", "aws"),
        ] {
            let read = super::arn_partition(role);
            assert_eq!(
                read, partition,
                "case {case:?}: expected {partition:?}, got {read:?}"
            );
        }
    }
}
