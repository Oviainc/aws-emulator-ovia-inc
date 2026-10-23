// SPDX-License-Identifier: CC0-1.0

//! @op CreateStorageLocation
//! Creates (or reports) this account's Elastic Beanstalk storage bucket for the
//! region the request was signed for. This is the ONLY file to touch for this
//! operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet
//! (see `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired
//! and the framework answered it from the unroutable-operation path — a query
//! `InvalidAction`, which is not an answer any AWS client can work with. The input,
//! output, error enum and schemas a generated stub would have carried are written
//! out below in the same shape as a rendered sibling
//! (`list_available_solution_stacks.rs`, the other hand-authored Unit-input
//! operation); `build.rs` picks the operation up from the `@op` line above and
//! wires it into the service's routing table.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]: the bucket this call
//! creates is recorded there, so a second call reports the one that already exists.
//!
//! ---
//!
//! # Modeled contract for `CreateStorageLocation`
//!
//! Creates a bucket in Amazon S3 to store application versions, logs, and other
//! files used by Elastic Beanstalk environments. The Elastic Beanstalk console and
//! EB CLI call this API the first time you create an environment in a region. If
//! the storage location already exists, `CreateStorageLocation` still returns the
//! bucket name but does not create a new bucket.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `S3Bucket` (S3Bucket, len 0..255) — The name of the Amazon S3 bucket created.
//!
//! ## Errors
//!
//! * `InsufficientPrivilegesException` (HTTP 403) — The specified account does not
//!   have sufficient privileges for one or more Amazon Web Services services.
//! * `S3SubscriptionRequiredException` (HTTP 400) — The specified account does not
//!   have a subscription to Amazon S3.
//! * `TooManyBucketsException` (HTTP 400) — The specified account has reached its
//!   limit of Amazon S3 buckets.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` and `#[smithy(..)]` below expand to invocations of the LOCAL
// declarative `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json");

/// `CreateStorageLocation` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct CreateStorageLocationInput;

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#CreateStorageLocationResultMessage")]
#[derive(Debug, Clone, Default)]
pub struct CreateStorageLocationOutput {
    pub s3_bucket: Option<String>,
}

/// Declared here rather than in `types/`: this is the only operation in the
/// service whose model reaches the shape.
#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#S3SubscriptionRequiredException")]
#[derive(Debug, Clone, Default)]
pub struct S3SubscriptionRequiredException {
    pub message: Option<String>,
}

// The operation's MODELED errors. Only the bucket quota is reachable here: this
// emulator owns the account's storage location, so there is no missing privilege
// and no absent S3 subscription — and a bucket that already exists is reported
// rather than refused.
#[smithy_macros::operation_error("com.amazonaws.elasticbeanstalk#CreateStorageLocation")]
#[derive(Debug)]
pub enum CreateStorageLocationErrorModeled {
    InsufficientPrivilegesException(InsufficientPrivilegesException),
    S3SubscriptionRequiredException(S3SubscriptionRequiredException),
    TooManyBucketsException(TooManyBucketsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_storage_location::CreateStorageLocationError`).
pub type CreateStorageLocationError =
    crate::scaffolded::Scaffolded<CreateStorageLocationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateStorageLocationErrorModeled> for CreateStorageLocationError {
    fn from(modeled: CreateStorageLocationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateStorageLocation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreateStorageLocation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateStorageLocation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateStorageLocation<Eng>
{
    type Input = CreateStorageLocationInput;
    type Output = CreateStorageLocationOutput;
    type Error = CreateStorageLocationError;

    const NAME: &'static str = "CreateStorageLocation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = CREATE_STORAGE_LOCATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        CREATE_STORAGE_LOCATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = &input;
        // One storage location per account per region, so the bucket's name is
        // derived from the region the request was signed for and this account.
        let region = crate::state::request_region(ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateStorageLocationError::internal_failure(
                "the Elastic Beanstalk store is unavailable",
            )
        })?;
        let mut store = state.lock();

        // "If the storage location already exists, CreateStorageLocation still
        // returns the bucket name but does not create a new bucket": the create is
        // idempotent, so the store holds the bucket at most once however many
        // times this is called.
        let bucket = crate::state::storage_bucket_name(&region);
        if !store.storage_buckets.contains(&bucket) {
            // "TooManyBucketsException: The specified account has reached its
            // limit of Amazon S3 buckets." Enforcing the quota is also what
            // bounds this store: the bucket's name is built from the region the
            // request was signed for, so the number of names a caller can mint
            // is capped rather than unbounded.
            if store.storage_buckets.len() >= crate::state::MAX_STORAGE_LOCATIONS {
                return Err(CreateStorageLocationErrorModeled::TooManyBucketsException(
                    TooManyBucketsException {
                        message: Some(format!(
                            "The number of Amazon S3 buckets in this account has reached the \
                             limit of {}.",
                            crate::state::MAX_STORAGE_LOCATIONS
                        )),
                    },
                )
                .into());
            }
            store.storage_buckets.insert(bucket.clone());
        }

        Ok(CreateStorageLocationOutput {
            s3_bucket: Some(bucket),
        })
    }
}
