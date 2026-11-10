// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 bucket a storage configuration records to: resolved, and granted to IVS,
//! where Amazon S3 OWNS it.
//!
//! `CreateStorageConfiguration` is documented as doing both — "used to enable recording to
//! Amazon S3. When a StorageConfiguration is created, IVS will modify the S3 bucketPolicy
//! of the provided bucket. This will ensure that IVS has sufficient permissions to write
//! content to the provided bucket" — and neither is a fact of this service's own store: the
//! bucket belongs to Amazon S3, and so does the policy on it. So a create RESOLVES the
//! bucket where S3 holds it before anything of this service is minted, and writes the grant
//! into that bucket's own policy afterwards, rather than keeping a name nothing backs.
//!
//! Amazon S3 is reached the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]), through the tree's one
//! construction of that client (`service_common::s3_client`), so nothing here opens a socket,
//! leaves the process, or names another service crate. Only S3's own ANSWER refuses
//! anything: a read that reports no such bucket is what refuses the request, while every
//! other outcome — an emulator assembled without Amazon S3 to ask, or any other failure to
//! ask it — establishes nothing about the bucket and so refuses nothing
//! (`services/common/src/network.rs` states which resolution is which).
//!
//! The one caller-supplied value that reaches a client's authority is the Region, which
//! [`super::request_region`] has already narrowed to the Region grammar the modelled ARN
//! patterns admit, so no request can re-point this client at a host of its own choosing.

/// The error code Amazon S3 answers a read of the policy of a bucket that carries none
/// with, which is "this bucket has no policy yet" rather than a failure to ask: the grant is
/// then the first statement the document holds.
const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";

/// The version every AWS resource policy declares, as Amazon S3's own bucket-policy
/// examples spell it.
const POLICY_VERSION: &str = "2012-10-17";

/// The action the grant allows: writing recorded content as an object of the bucket, which
/// is the permission `CreateStorageConfiguration` documents it ensures IVS has ("sufficient
/// permissions to write content to the provided bucket").
const RECORDING_ACTION: &str = "s3:PutObject";

/// Whether Amazon S3 reports holding no bucket under `name` in `region`.
///
/// Asked with `HeadBucket`, the operation S3 documents for exactly this ("You can use this
/// operation to determine if a bucket exists"): its `NotFound` answer is S3 saying the
/// bucket is not there. Every other failure leaves the question unanswered, so it reports
/// the bucket as present and refuses nothing.
///
/// Call it with NO guard of this service's store held: it awaits.
pub async fn recording_bucket_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> bool {
    let answer = s3_client(ctx, region).head_bucket().bucket(name).send().await;
    match answer {
        Ok(_) => false,
        Err(error)
            if error.as_service_error().is_some_and(
                ::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found,
            ) =>
        {
            true
        }
        Err(error) => {
            unresolved(name, &error);
            false
        }
    }
}

/// Write the grant IVS needs to record into `bucket` into that bucket's own policy, where
/// Amazon S3 keeps it.
///
/// The grant is one statement: the composite recorder of this Region may write objects of
/// the bucket over a secure transport ([`recording_grant_statement`]). It is MERGED into
/// whatever policy the bucket already carries — the statement for this Region's recorder
/// replaces the one a previous create wrote and every other statement is left as it was —
/// so a second storage configuration over one bucket leaves the bucket granting exactly
/// once, and a policy the bucket's owner wrote keeps its own statements.
///
/// Call it with NO guard of this service's store held: it awaits.
pub async fn grant_recording_access(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) {
    let client = s3_client(ctx, region);
    let carried = match client.get_bucket_policy().bucket(bucket).send().await {
        Ok(answer) => answer
            .policy
            .as_deref()
            .and_then(|text| ::serde_json::from_str::<::serde_json::Value>(text).ok()),
        Err(error) => {
            // A bucket that carries no policy yet is the ordinary case of a first create,
            // not a failure: the grant becomes the document's first statement. Anything
            // else is a bucket this emulator could not ask about, which the put below
            // reports.
            if ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error)
                != Some(NO_SUCH_BUCKET_POLICY_CODE)
            {
                ungranted(bucket, &error);
            }
            None
        }
    };
    let document = merged_recording_grant(carried, region, bucket);
    let document = match ::serde_json::to_string(&document) {
        Ok(document) => document,
        Err(error) => {
            ungranted(bucket, &error);
            return;
        }
    };
    if let Err(error) = client
        .put_bucket_policy()
        .bucket(bucket)
        .policy(document)
        .send()
        .await
    {
        ungranted(bucket, &error);
    }
}

/// The policy `bucket` is to carry: the one it carries now with the recording grant of
/// `region` merged in.
///
/// A statement already granting this Region's recorder is REPLACED rather than added beside,
/// so the grant is written once however often a storage configuration is created over one
/// bucket; the grant is identified by the principal it names, which is what makes two
/// creates in one Region the same grant and one in another Region a second one. A document
/// that is not an object holding a statement list is not one this service can merge into, so
/// the grant stands alone in a fresh document — the state the bucket is in before any policy
/// was written; Amazon S3 admits only a policy document on the write, so that is the shape a
/// bucket can actually be carrying.
fn merged_recording_grant(
    carried: Option<::serde_json::Value>,
    region: &str,
    bucket: &str,
) -> ::serde_json::Value {
    let grant = recording_grant_statement(region, bucket);
    let principal = recording_service_principal(region);
    let mut document = match carried {
        Some(::serde_json::Value::Object(document)) => document,
        _ => ::serde_json::Map::new(),
    };
    let mut statements: Vec<::serde_json::Value> = match document.get("Statement") {
        Some(::serde_json::Value::Array(statements)) => statements.clone(),
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement.clone()],
        _ => Vec::new(),
    };
    statements.retain(|statement| !grants_service(statement, &principal));
    statements.push(grant);
    document.insert(
        "Version".to_string(),
        ::serde_json::Value::String(POLICY_VERSION.to_string()),
    );
    document.insert("Statement".to_string(), ::serde_json::Value::Array(statements));
    ::serde_json::Value::Object(document)
}

/// Whether `statement` is a grant to the service principal `principal` — the statement this
/// service's own grant replaces.
fn grants_service(statement: &::serde_json::Value, principal: &str) -> bool {
    statement
        .get("Principal")
        .and_then(|named| named.get("Service"))
        .is_some_and(|service| match service {
            ::serde_json::Value::String(named) => named == principal,
            ::serde_json::Value::Array(named) => named
                .iter()
                .any(|named| named.as_str() == Some(principal)),
            _ => false,
        })
}

/// The one statement the grant adds: this Region's composite recorder may write objects of
/// `bucket`, over a secure transport.
///
/// `Resource` is every object of the bucket, which is where a recording lands and the only
/// thing the write needs; the condition is Amazon S3's own published form for requiring
/// TLS, so the grant does not reach a request made in the clear.
fn recording_grant_statement(region: &str, bucket: &str) -> ::serde_json::Value {
    ::serde_json::json!({
        "Effect": "Allow",
        "Principal": { "Service": recording_service_principal(region) },
        "Action": [RECORDING_ACTION],
        "Resource": format!("arn:aws:s3:::{bucket}/*"),
        "Condition": { "Bool": { "aws:SecureTransport": "true" } },
    })
}

/// The service principal IVS composites and records a stage as, in `region`: the identity
/// the bucket's policy grants, built from the Region alone.
fn recording_service_principal(region: &str) -> String {
    format!("ivs-composite.{region}.amazonaws.com")
}

/// Trace a bucket this emulator could not ask Amazon S3 about, which is a deployment with no
/// such service mounted rather than a bucket reported missing.
///
/// The caller's own value is rendered in its escaped `Debug` form, so a name carrying a
/// newline cannot forge a log line.
fn unresolved<E: ::std::error::Error + 'static>(name: &str, error: &E) {
    ::tracing::warn!(
        bucket_name = ?name,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a storage configuration's bucket could not be resolved against Amazon S3"
    );
}

/// Trace a bucket whose policy this emulator could not amend, for the same reason and in the
/// same form.
fn ungranted<E: ::std::error::Error + 'static>(name: &str, error: &E) {
    ::tracing::warn!(
        bucket_name = ?name,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "the recording grant of a storage configuration could not be written into the \
         bucket's policy"
    );
}

/// Amazon S3's client for `region`, over the in-process callback into this same emulator.
///
/// The tree's ONE construction of that client (`service_common::s3_client`), so this crate
/// configures the hop no differently from every other service that reaches the account's
/// object store: the callback's HTTP client and credentials, path-style addressing, and none
/// of what a network client needs and this one cannot have.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    ::service_common::s3_client::s3_client(ctx.callback(), region)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the merge leaves the bucket carrying, over the policies a bucket can already
    /// have: none at all, one a client wrote, one this grant already wrote (in this Region
    /// and in another), and a document that is not a policy this service can read.
    #[test]
    fn the_grant_is_merged_into_whatever_policy_the_bucket_carries() {
        let bucket = "ivs-recordings";
        let region = "us-west-2";
        let principal = recording_service_principal(region);
        let elsewhere = recording_service_principal("eu-west-1");
        let foreign = ::serde_json::json!({
            "Sid": "OwnerRead",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
            "Action": ["s3:GetObject"],
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
        });
        // (case, the policy the bucket carries, the statements left beside the grant)
        let cases: Vec<(&str, Option<::serde_json::Value>, Vec<::serde_json::Value>)> = vec![
            ("a bucket carrying no policy at all", None, Vec::new()),
            (
                "a policy of the bucket owner's own",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": [foreign.clone()],
                })),
                vec![foreign.clone()],
            ),
            (
                "a policy carrying one statement rather than a list",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": foreign.clone(),
                })),
                vec![foreign.clone()],
            ),
            (
                "a policy this grant already wrote",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": [recording_grant_statement(region, bucket)],
                })),
                Vec::new(),
            ),
            (
                "a policy granting another Region's recorder",
                Some(::serde_json::json!({
                    "Version": POLICY_VERSION,
                    "Statement": [recording_grant_statement("eu-west-1", bucket)],
                })),
                vec![recording_grant_statement("eu-west-1", bucket)],
            ),
            (
                "a document that is not a policy",
                Some(::serde_json::json!("not a policy")),
                Vec::new(),
            ),
        ];

        for (case, carried, kept) in cases {
            let merged = merged_recording_grant(carried, region, bucket);
            assert_eq!(
                merged["Version"],
                ::serde_json::json!(POLICY_VERSION),
                "case {case:?}: the merged policy must declare the policy version, \
                 got {merged}"
            );
            let statements = merged["Statement"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the merge must hold a statement list"))
                .clone();
            let granting: Vec<&::serde_json::Value> = statements
                .iter()
                .filter(|statement| grants_service(statement, &principal))
                .collect();
            assert_eq!(
                granting.len(),
                1,
                "case {case:?}: the bucket must grant this Region's recorder exactly once, \
                 got {statements:?}"
            );
            let grant = granting[0];
            assert_eq!(
                *grant,
                recording_grant_statement(region, bucket),
                "case {case:?}: the grant must be the recording statement, got {grant}"
            );
            let beside: Vec<::serde_json::Value> = statements
                .iter()
                .filter(|statement| !grants_service(statement, &principal))
                .cloned()
                .collect();
            assert_eq!(
                beside, kept,
                "case {case:?}: every other statement must be left as it was, \
                 got {beside:?} against {kept:?}"
            );
            assert!(
                beside
                    .iter()
                    .all(|statement| !grants_service(statement, &principal)),
                "case {case:?}: no second grant to {principal} may remain, got {beside:?}"
            );
            assert!(
                !grants_service(grant, &elsewhere),
                "case {case:?}: the grant must name this Region's recorder, got {grant}"
            );
        }
    }

    /// The grant states the permission, the principal, the objects and the transport
    /// condition, with the Region and the bucket of the request in them and nothing else.
    #[test]
    fn the_grant_states_the_recording_permission() {
        // (case, the Region the configuration is created in, the bucket it records to)
        let cases = [
            ("the Region of the recording", "us-west-2", "ivs-recordings"),
            ("another Region", "eu-west-1", "ivs.recordings.2026"),
            ("a Region of another partition's grammar", "ap-south-1", "abc"),
        ];
        for (case, region, bucket) in cases {
            let grant = recording_grant_statement(region, bucket);
            assert_eq!(
                grant["Effect"],
                ::serde_json::json!("Allow"),
                "case {case:?}: the grant must allow, got {grant}"
            );
            assert_eq!(
                grant["Principal"]["Service"],
                ::serde_json::json!(format!("ivs-composite.{region}.amazonaws.com")),
                "case {case:?}: the grant must name the Region's composite recorder, \
                 got {grant}"
            );
            assert_eq!(
                grant["Resource"],
                ::serde_json::json!(format!("arn:aws:s3:::{bucket}/*")),
                "case {case:?}: the grant must cover every object of the bucket, got {grant}"
            );
            assert_eq!(
                grant["Condition"]["Bool"]["aws:SecureTransport"],
                ::serde_json::json!("true"),
                "case {case:?}: the grant must require a secure transport, got {grant}"
            );
            let actions = grant["Action"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the grant lists its actions"));
            assert!(
                actions.contains(&::serde_json::json!(RECORDING_ACTION)),
                "case {case:?}: the grant must allow the recording write, got {grant}"
            );
        }
    }
}
