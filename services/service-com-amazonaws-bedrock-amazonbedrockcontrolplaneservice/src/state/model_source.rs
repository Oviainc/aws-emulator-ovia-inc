// SPDX-License-Identifier: CC0-1.0

//! Whether the Amazon S3 location a model source names satisfies the pre-condition the
//! member carrying it states.
//!
//! `CreateCustomModel`'s `modelSourceConfig` states one: "The Amazon S3 URI in the model
//! source must be for the Amazon-managed Amazon S3 bucket containing your model artifacts"
//! (`smithy_query(query="doc",shape="CreateCustomModelRequest",member="modelSourceConfig")`,
//! and the operation's own documentation repeats it). So the member's FORM — a
//! syntactically valid `s3://` URI, which its `@pattern` already enforces — is not the
//! whole requirement: WHOSE bucket the URI names is part of it, and a bucket the account
//! itself holds does not satisfy it however readable that bucket is and whatever role the
//! request passes for it.
//!
//! Which buckets are Amazon's own is a question the object store ANSWERS, asked over the
//! in-process callback every cross-service call of an emulated service uses
//! ([`super::model_invocation_run::s3_client`] — no socket, no outbound request, no host
//! resolution; the caller's URI reaches it only as a bucket name). A bucket AWS publishes
//! exists in S3's global namespace while being none of "the buckets in your account", so a
//! listing of the account's buckets leaves it out and a HEAD of it still resolves — which
//! is the difference read here:
//!
//! * the account HOLDS the bucket -> the caller's own bucket, so the pre-condition fails
//!   ([`UNSUPPORTED_S3_URI`]);
//! * the object store holds NO bucket of that name -> there is nothing the URI could name,
//!   which Amazon Bedrock states as the input it could not take
//!   ([`unsupported_input_message`]);
//! * the bucket resolves and is not one of the account's -> it is AWS's own, and the
//!   pre-condition holds.
//!
//! Refused only on what the object store ANSWERED: an emulator assembled with no Amazon S3
//! to ask, or an answer this could not read, leaves the source admitted, because a refusal
//! has to be grounded — the same rule the rest of this service's cross-service decisions
//! follow ([`crate::state::job_pass_role_denial`],
//! [`super::model_invocation_run`]).

use crate::ops::types::ModelDataSource;

use super::model_invocation_run::{names_nothing, prefix_address, s3_client};

/// What Amazon Bedrock answers for a model source whose Amazon S3 URI names a bucket that
/// EXISTS and is not the Amazon-managed one: RECORDED behaviour, and a sentence with no
/// variable part — it names neither the URI nor the bucket, so there is nothing of the
/// request to fill in. It is answered as the modelled `ValidationException`
/// (`@httpError(400)`).
pub const UNSUPPORTED_S3_URI: &str = "S3 URI not supported";

/// What Amazon Bedrock answers for a model source naming a bucket that does not exist: the
/// input it could not take, which is the BUCKET the URI named.
///
/// RECORDED behaviour, and a message BUILT FROM THE REQUEST rather than a constant: the
/// corpus records the bucket the submitted URI carried, quoted, in exactly this sentence —
/// so the rule is "name the bucket the URI names", not one recorded bucket. Answered as
/// the modelled `ValidationException` (`@httpError(400)`), the same error the
/// Amazon-managed-bucket refusal above takes: the two are the same member's pre-condition,
/// worded for what the object store said about it.
#[must_use]
pub fn unsupported_input_message(bucket: &str) -> String {
    format!("Unsupported input passed: '{bucket}'.")
}

/// Whose bucket a name resolves to, as far as the object store said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// One of the buckets in the caller's own account.
    Account,
    /// A bucket that resolves and is none of the account's: AWS's own.
    Aws,
    /// The object store holds no bucket of that name at all.
    Nothing,
    /// Nothing is known: no object store answered the question.
    Unknown,
}

/// Why the model source `source` does not satisfy the Amazon-managed-bucket pre-condition,
/// or `None` when it satisfies it (and when nothing answered the question).
///
/// The sentence is the one the object store's answer calls for — the bucket exists and is
/// the caller's own ([`UNSUPPORTED_S3_URI`]), or there is no such bucket at all
/// ([`unsupported_input_message`]). A caller that gets either has had nothing written on
/// its behalf, because this stands in front of the handler's own state write.
pub async fn unsupported_model_source(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: &ModelDataSource,
) -> Option<String> {
    let ModelDataSource::s3DataSource(s3) = source;
    // A value of the S3 scheme that names no bucket is one this check has nothing to ask
    // about; the member's own `@pattern` refuses such a value before the handler runs.
    let (bucket, _prefix) = prefix_address(&s3.s3_uri)?;
    match holder_of(ctx, region, &bucket).await {
        Holder::Account => Some(UNSUPPORTED_S3_URI.to_string()),
        Holder::Nothing => Some(unsupported_input_message(&bucket)),
        Holder::Aws | Holder::Unknown => None,
    }
}

/// Ask this emulator's own Amazon S3 whose bucket `bucket` is.
///
/// Two questions, in the order that makes each answer mean something: does the bucket
/// resolve AT ALL (a HEAD of it), and is it one of the buckets in this account (a listing
/// of them). A bucket that resolves and is not listed is AWS's own, which is the only kind
/// of Amazon-managed bucket there is to name.
async fn holder_of(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Holder {
    let client = s3_client(ctx, region);
    if let Err(error) = client.head_bucket().bucket(bucket).send().await {
        if names_nothing(&error) {
            // S3 says it holds no bucket of that name: the verdict is grounded.
            return Holder::Nothing;
        }
        // Any other answer is not S3 reporting an absent bucket, so it establishes nothing
        // about who holds it — and it is worth a warning, because an emulator with no
        // object store mounted to ask is one where this pre-condition cannot be decided.
        // The caller's own value is rendered in its escaped `Debug` form, so a bucket name
        // carrying a newline cannot forge a log line.
        ::tracing::warn!(
            bucket = ?bucket,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the model source's bucket could not be resolved in this emulator's Amazon S3"
        );
        return Holder::Unknown;
    }
    match client.list_buckets().send().await {
        // "The buckets in your account": a bucket AWS publishes is not among them, which
        // is what makes it Amazon's rather than the caller's.
        Ok(answer) => {
            if answer
                .buckets()
                .iter()
                .any(|held| held.name() == Some(bucket))
            {
                Holder::Account
            } else {
                Holder::Aws
            }
        }
        Err(error) => {
            ::tracing::warn!(
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the account's buckets could not be listed in this emulator's Amazon S3"
            );
            Holder::Unknown
        }
    }
}
