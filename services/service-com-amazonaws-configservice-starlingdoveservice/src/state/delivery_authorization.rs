// SPDX-License-Identifier: CC0-1.0

//! Whether Config MAY deliver into the Amazon S3 bucket a delivery channel names.
//!
//! A delivery channel is only usable if Config itself can reach the bucket behind it:
//! Config delivers as its own service principal, so the permission has to come from the
//! BUCKET'S OWN policy, and `PutDeliveryChannel` models the refusal of a bucket whose
//! policy withholds it — `InsufficientDeliveryPolicyException` ("Your Amazon S3 bucket
//! policy does not allow Config to write to it"). So a channel is accepted only when the
//! policy in force on the named bucket allows the Config service principal each S3 call the
//! service makes on it before it will take the channel ([`DELIVERY_CHECKS`]), and the
//! refusal names the one that was denied, the way the service words it.
//!
//! What the bucket holds is read from S3 ITSELF, over the same in-process callback the
//! recorder observes resources through ([`super::s3_client`]), so this crate names no
//! other service crate; and the decision the policy gives is the workspace's ONE
//! evaluation of an IAM policy document (`service_common::iam_authorization`) rather than
//! a second reading of that grammar here. A read that establishes no policy — S3 answering
//! nothing, or answering about a bucket it does not hold — grounds no refusal
//! ([`DeliveryAuthorization::Unanswered`]): this decides what the POLICY says and nothing
//! else, a bucket's EXISTENCE being the separate requirement `NoSuchBucketException`
//! carries.
//!
//! The condition context a delivery carries is the context this evaluation passes: the
//! account the recorder delivers for, the ARN of the recorder it delivers for, and the
//! canned ACL Config writes a delivered object with — the three keys the bucket policy AWS
//! documents for a delivery channel constrains. A statement conditioned on any other key
//! cannot be decided, and the shared evaluation reads an undecided statement as granting
//! nothing.

use ::service_common::iam_authorization::{resource_decision, Decision, Request};

use super::{recorded_resource_arn, s3_client, S3_BUCKET_RESOURCE_TYPE};

/// The service principal Config delivers as, which is the principal a delivery bucket's
/// policy has to name.
pub const CONFIG_SERVICE_PRINCIPAL: &str = "config.amazonaws.com";

/// The S3 calls Config makes on a delivery channel's bucket before it will take the
/// channel, in the order it makes them: it reads the bucket's ACL, then writes a delivered
/// object. Each is one entry of [`DELIVERY_CHECKS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCheck {
    /// `s3:GetBucketAcl` on the bucket itself (`arn:aws:s3:::<bucket>`), which the bucket
    /// policy AWS documents for a delivery channel grants in its
    /// `AWSConfigBucketPermissionsCheck` statement.
    BucketAcl,
    /// `s3:PutObject` on the object Config WRITES to check that it can deliver into the
    /// bucket (`ConfigWritabilityCheckFile` under this account's own delivery path,
    /// [`super::delivery_writability_check_key`]); the documented policy grants it in its
    /// `AWSConfigBucketDelivery` statement.
    ObjectWrite,
}

/// The checks a delivery channel's bucket has to pass, in the order the service makes them
/// — which is the order a refusal names them in, since the first one denied is the one
/// reported.
///
/// The two are the calls the service's own refusals NAME ("unable to get s3 bucket acl",
/// "unable to write to bucket"); the documented policy's third statement, the
/// `AWSConfigBucketExistenceCheck` grant of `s3:ListBucket`, is not one of them, so a
/// bucket whose policy withholds that alone is still taken.
pub const DELIVERY_CHECKS: [DeliveryCheck; 2] =
    [DeliveryCheck::BucketAcl, DeliveryCheck::ObjectWrite];

impl DeliveryCheck {
    /// The IAM action this check needs.
    pub fn action(self) -> &'static str {
        match self {
            Self::BucketAcl => "s3:GetBucketAcl",
            Self::ObjectWrite => "s3:PutObject",
        }
    }

    /// How the service's refusal names this check once it is denied: the clause after
    /// "unable to" in [`insufficient_delivery_policy_message`]. The object write names the
    /// key prefix the channel asked for with it, since that prefix is what decides which
    /// key the write would have gone to.
    fn denied_clause(self, key_prefix: Option<&str>) -> String {
        match self {
            Self::BucketAcl => "get s3 bucket acl".to_string(),
            Self::ObjectWrite => {
                let prefix = key_prefix
                    .map(str::trim)
                    .filter(|prefix| !prefix.is_empty())
                    .unwrap_or(NO_KEY_PREFIX);
                format!("write to bucket, provided s3 key prefix is '{prefix}'")
            }
        }
    }
}

/// The canned ACL Config writes a delivered object with, which the documented delivery
/// statement conditions on (`s3:x-amz-acl` equal to it).
const DELIVERY_OBJECT_ACL: &str = "bucket-owner-full-control";

/// The condition key naming the account whose Config delivers, and the one naming the
/// resource of Config's that the delivery is made for — the two the documented bucket
/// policy constrains beside the ACL above. IAM compares a condition key's NAME
/// case-insensitively, so one spelling of each serves a policy written in any of them.
const SOURCE_ACCOUNT_KEY: &str = "AWS:SourceAccount";
/// See [`SOURCE_ACCOUNT_KEY`].
const SOURCE_ARN_KEY: &str = "AWS:SourceArn";
/// See [`DELIVERY_OBJECT_ACL`].
const OBJECT_ACL_KEY: &str = "s3:x-amz-acl";

/// The code S3 answers for a bucket that carries no policy at all, which is a bucket whose
/// policy allows Config nothing.
const NO_SUCH_BUCKET_POLICY: &str = "NoSuchBucketPolicy";

/// The `s3KeyPrefix` a channel that configures none is reported with in
/// [`insufficient_delivery_policy_message`], as the service words that sentence for a
/// channel carrying no prefix.
const NO_KEY_PREFIX: &str = "null";

/// What the bucket behind a delivery channel says about Config delivering into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAuthorization {
    /// The policy in force on the bucket allows Config every check a delivery channel's
    /// bucket has to pass.
    Permitted,
    /// The bucket's policy does not allow Config the named check — a bucket carrying no
    /// policy at all allows it the first of them — which is
    /// `InsufficientDeliveryPolicyException`, worded after that check.
    Denied(DeliveryCheck),
    /// S3 established no policy for the bucket: it answered neither a policy nor that the
    /// bucket carries none, so there is nothing for this decision to read.
    Unanswered,
}

/// How Config words `InsufficientDeliveryPolicyException`: the bucket it could not deliver
/// into, and the call on it the bucket's policy denied — which is what tells a client WHICH
/// grant is missing.
pub fn insufficient_delivery_policy_message(
    bucket: &str,
    denied: DeliveryCheck,
    key_prefix: Option<&str>,
) -> String {
    format!(
        "Insufficient delivery policy to s3 bucket: {bucket}, unable to {}.",
        denied.denied_clause(key_prefix)
    )
}

/// What the bucket `bucket` says about Config delivering into it under `key_prefix`, read
/// from S3 over the in-process callback.
///
/// `source_arn` is the resource of Config's the delivery is made for — the account's
/// customer managed configuration recorder — for a policy that conditions its grant on
/// it; a channel put before any recorder names one carries none.
pub async fn delivery_authorization(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    bucket: &str,
    key_prefix: Option<&str>,
    source_arn: Option<&str>,
) -> DeliveryAuthorization {
    let answered = s3_client(callback, region)
        .get_bucket_policy()
        .bucket(bucket)
        .send()
        .await;
    let document = match answered {
        Ok(answer) => answer.policy.unwrap_or_default(),
        Err(error) => {
            use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
            return match error.code() {
                // A bucket carrying no policy grants Config nothing, so the first check it
                // makes on the bucket is the one denied.
                Some(NO_SUCH_BUCKET_POLICY) => {
                    DeliveryAuthorization::Denied(DELIVERY_CHECKS[0])
                }
                _ => DeliveryAuthorization::Unanswered,
            };
        }
    };
    match denied_delivery_check(&document, region, bucket, key_prefix, source_arn) {
        Some(denied) => DeliveryAuthorization::Denied(denied),
        None => DeliveryAuthorization::Permitted,
    }
}

/// The FIRST check the bucket policy `document` denies the Config service principal — each
/// asked about the resource that call is made on, in the condition context a delivery
/// carries — or `None` for a policy that allows them all.
pub fn denied_delivery_check(
    document: &str,
    region: &str,
    bucket: &str,
    key_prefix: Option<&str>,
    source_arn: Option<&str>,
) -> Option<DeliveryCheck> {
    let account_id = ::emulator_server::emulator_account_id().to_string();
    let principals = [CONFIG_SERVICE_PRINCIPAL.to_string()];
    // A bucket this crate cannot name has no resource for a statement to grant on, so
    // nothing permits a delivery into it.
    let bucket_arn = bucket_arn(bucket, region)?;
    // The object the write is judged against: the one Config itself puts in the bucket to
    // check that it can deliver there, under this account's own delivery path and the
    // channel's own prefix.
    let object_arn = format!(
        "{bucket_arn}/{}",
        super::delivery_writability_check_key(key_prefix, &account_id)
    );
    let mut context = vec![
        (SOURCE_ACCOUNT_KEY.to_string(), account_id),
        (OBJECT_ACL_KEY.to_string(), DELIVERY_OBJECT_ACL.to_string()),
    ];
    if let Some(source_arn) = source_arn {
        context.push((SOURCE_ARN_KEY.to_string(), source_arn.to_string()));
    }
    DELIVERY_CHECKS.into_iter().find(|check| {
        let resource = match check {
            DeliveryCheck::BucketAcl => &bucket_arn,
            DeliveryCheck::ObjectWrite => &object_arn,
        };
        let request = Request::new(check.action(), resource, &principals).with_context(&context);
        resource_decision(document, &request) != Decision::Allowed
    })
}

/// The ARN of a delivery channel's bucket (`arn:aws:s3:::<bucket>`), through the ONE place
/// this crate resolves an S3 bucket to its ARN — so the resource a bucket policy is judged
/// against is spelled exactly as the recorded inventory spells the same bucket. `None` for
/// a channel naming no bucket at all.
fn bucket_arn(bucket: &str, region: &str) -> Option<String> {
    recorded_resource_arn(S3_BUCKET_RESOURCE_TYPE, bucket, region)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN of [`BUCKET`], which every case below writes its policy against.
    fn arn() -> String {
        bucket_arn(BUCKET, REGION).unwrap_or_default()
    }

    /// The bucket and Region every case below is about.
    const BUCKET: &str = "config-delivery-bucket";
    /// See [`BUCKET`].
    const REGION: &str = "us-west-2";

    /// The bucket policy AWS documents for a delivery channel, as a client writes it:
    /// `statements` are the `(Sid, Action, Resource)` triples it carries, each an `Allow`
    /// for the Config service principal conditioned the way the documented policy
    /// conditions it.
    fn policy(statements: &[(&str, &str, String)]) -> String {
        let account = ::emulator_server::emulator_account_id();
        let rendered: Vec<::serde_json::Value> = statements
            .iter()
            .map(|(sid, action, resource)| {
                ::serde_json::json!({
                    "Sid": sid,
                    "Effect": "Allow",
                    "Principal": { "Service": CONFIG_SERVICE_PRINCIPAL },
                    "Action": action,
                    "Resource": resource,
                    "Condition": { "StringEquals": { "AWS:SourceAccount": account } },
                })
            })
            .collect();
        ::serde_json::json!({ "Version": "2012-10-17", "Statement": rendered }).to_string()
    }

    /// The three statements the documented policy carries, in full.
    fn documented_statements() -> Vec<(&'static str, &'static str, String)> {
        let bucket_arn = arn();
        vec![
            ("AWSConfigBucketPermissionsCheck", "s3:GetBucketAcl", bucket_arn.clone()),
            ("AWSConfigBucketExistenceCheck", "s3:ListBucket", bucket_arn.clone()),
            ("AWSConfigBucketDelivery", "s3:PutObject", format!("{bucket_arn}/*")),
        ]
    }

    /// A policy is sufficient only while it grants Config every call it makes on the
    /// bucket, and the check it DENIES is the one reported: the rows drop each statement of
    /// the documented policy in turn, and drop the policy altogether.
    #[test]
    fn the_check_a_policy_denies_is_the_one_reported() {
        let documented = documented_statements();
        assert_eq!(
            denied_delivery_check(&policy(&documented), REGION, BUCKET, None, None),
            None,
            "the documented policy denies no check, got a refusal"
        );
        // Dropping the ACL grant denies the first check, dropping the delivery grant the
        // second, and dropping the existence grant denies neither: the service's own
        // refusals name only those two calls.
        let cases: [(&str, &str, Option<DeliveryCheck>); 3] = [
            (
                "no permissions check",
                "AWSConfigBucketPermissionsCheck",
                Some(DeliveryCheck::BucketAcl),
            ),
            (
                "no delivery grant",
                "AWSConfigBucketDelivery",
                Some(DeliveryCheck::ObjectWrite),
            ),
            ("no existence check", "AWSConfigBucketExistenceCheck", None),
        ];
        for (case, dropped, denied) in cases {
            let kept: Vec<(&str, &str, String)> = documented
                .iter()
                .filter(|(sid, _, _)| *sid != dropped)
                .cloned()
                .collect();
            let document = policy(&kept);
            assert_eq!(
                denied_delivery_check(&document, REGION, BUCKET, None, None),
                denied,
                "case {case:?}: expected {denied:?} to be denied, got otherwise with \
                 {document}"
            );
        }
        assert_eq!(
            denied_delivery_check("", REGION, BUCKET, None, None),
            Some(DeliveryCheck::BucketAcl),
            "a bucket carrying no policy denies the first check Config makes"
        );
    }

    /// The delivery statement has to cover the KEY Config WRITES to check the bucket, which
    /// is `ConfigWritabilityCheckFile` under this account's own delivery path and the
    /// channel's own prefix: the rows differ along the prefix the channel configures and the
    /// resource the policy names.
    #[test]
    fn the_delivery_statement_has_to_cover_the_key_config_writes() {
        let bucket_arn = arn();
        let account = ::emulator_server::emulator_account_id();
        let cases: [(&str, Option<&str>, String, bool); 6] = [
            (
                "every object of the bucket, no prefix",
                None,
                format!("{bucket_arn}/*"),
                true,
            ),
            (
                "the account's own Config path, no prefix",
                None,
                format!("{bucket_arn}/AWSLogs/{account}/Config/*"),
                true,
            ),
            (
                "the account's own Config path behind the channel's prefix",
                Some("audit"),
                format!("{bucket_arn}/audit/AWSLogs/{account}/Config/*"),
                true,
            ),
            (
                "the check object itself",
                None,
                format!("{bucket_arn}/AWSLogs/{account}/Config/ConfigWritabilityCheckFile"),
                true,
            ),
            (
                "one Region's delivered objects alone, which the check is not among",
                None,
                format!("{bucket_arn}/AWSLogs/{account}/Config/{REGION}/*"),
                false,
            ),
            (
                "another account's Config path",
                None,
                format!("{bucket_arn}/AWSLogs/999999999999/Config/*"),
                false,
            ),
        ];

        for (case, prefix, resource, permitted) in cases {
            let mut statements = documented_statements();
            statements.pop();
            statements.push(("AWSConfigBucketDelivery", "s3:PutObject", resource));
            let document = policy(&statements);
            let denied = denied_delivery_check(&document, REGION, BUCKET, prefix, None);
            assert_eq!(
                denied.is_none(),
                permitted,
                "case {case:?}: expected permitted={permitted}, got {denied:?} with \
                 {document}"
            );
        }
    }

    /// The grant has to name CONFIG: a policy granting the same three actions to another
    /// principal, or denying them to Config, permits no delivery.
    #[test]
    fn only_a_grant_to_config_permits_a_delivery() {
        let bucket_arn = arn();
        let cases: [(&str, ::serde_json::Value, &str, bool); 4] = [
            (
                "the Config service principal",
                ::serde_json::json!({ "Service": CONFIG_SERVICE_PRINCIPAL }),
                "Allow",
                true,
            ),
            (
                "every principal",
                ::serde_json::json!("*"),
                "Allow",
                true,
            ),
            (
                "another service principal",
                ::serde_json::json!({ "Service": "cloudtrail.amazonaws.com" }),
                "Allow",
                false,
            ),
            (
                "Config, denied",
                ::serde_json::json!({ "Service": CONFIG_SERVICE_PRINCIPAL }),
                "Deny",
                false,
            ),
        ];

        for (case, principal, effect, permitted) in cases {
            let document = ::serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": effect,
                    "Principal": principal,
                    "Action": ["s3:GetBucketAcl", "s3:ListBucket", "s3:PutObject"],
                    "Resource": [bucket_arn.clone(), format!("{bucket_arn}/*")],
                }],
            })
            .to_string();
            let denied = denied_delivery_check(&document, REGION, BUCKET, None, None);
            assert_eq!(
                denied.is_none(),
                permitted,
                "case {case:?}: expected permitted={permitted}, got {denied:?} with \
                 {document}"
            );
        }
    }

    /// The sentence the refusal carries names the bucket and the CALL the policy denied:
    /// the ACL read as itself, and the object write with the key prefix the channel asked
    /// for, which is what decides the key that write would have gone to.
    #[test]
    fn the_refusal_names_the_bucket_and_the_denied_call() {
        let cases: [(&str, DeliveryCheck, Option<&str>, &str); 4] = [
            (
                "the ACL read",
                DeliveryCheck::BucketAcl,
                None,
                "get s3 bucket acl",
            ),
            (
                "the ACL read, with a prefix configured",
                DeliveryCheck::BucketAcl,
                Some("audit"),
                "get s3 bucket acl",
            ),
            (
                "the object write, no prefix",
                DeliveryCheck::ObjectWrite,
                None,
                "write to bucket, provided s3 key prefix is 'null'",
            ),
            (
                "the object write, a prefix",
                DeliveryCheck::ObjectWrite,
                Some("audit"),
                "write to bucket, provided s3 key prefix is 'audit'",
            ),
        ];
        for (case, denied, prefix, clause) in cases {
            let message = insufficient_delivery_policy_message(BUCKET, denied, prefix);
            let expected =
                format!("Insufficient delivery policy to s3 bucket: {BUCKET}, unable to {clause}.");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
