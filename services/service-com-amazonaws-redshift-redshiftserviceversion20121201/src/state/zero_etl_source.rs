// SPDX-License-Identifier: CC0-1.0

//! Whether the zero-ETL SOURCE a `CreateIntegration` names EXISTS: a fact the
//! service that OWNS the source answers, not this one.
//!
//! `CreateIntegration$SourceArn` is "the Amazon Resource Name (ARN) of the
//! database to use as the source for replication", and its modelled pattern
//! `^arn:aws[a-z\-]*:(s3|dynamodb):.*:.*:[a-zA-Z0-9._\-/]+$`
//! (`smithy_query(query="operation", operation="CreateIntegration")`) admits a
//! resource of Amazon DynamoDB or of Amazon S3 — never one of this service. So
//! whether there is such a table or bucket is a question only those services can
//! answer, and it is asked the way every emulated service reaches another: the
//! AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which opens no socket
//! and leaves no part of this process.
//!
//! Nothing is invented and nothing is assumed from silence, exactly as in
//! [`super::serverless`]: an owner that answers that it holds no such resource
//! settles the question ([`SourceResolution::Absent`]) and the create is refused;
//! one that cannot be asked at all — an emulator with no DynamoDB mounted beside
//! this one knows of no table either way — leaves it UNDECIDED
//! ([`SourceResolution::Unresolvable`]), and the create is admitted rather than
//! refused on the strength of a peer that never answered.

/// The service namespace of a zero-ETL source held by Amazon DynamoDB.
const DYNAMODB_ARN_VENDOR: &str = "dynamodb";

/// The service namespace of a zero-ETL source held by Amazon S3.
const S3_ARN_VENDOR: &str = "s3";

/// The resource type of the DynamoDB resource a zero-ETL integration replicates:
/// a TABLE. The same vendor's other resource types (a stream, an index, a backup)
/// are not sources, so an ARN naming one is left undecided rather than resolved
/// against a table of that name.
const DYNAMODB_TABLE_RESOURCE_TYPE: &str = "table";

/// How many pages of a bucket listing are read before the question is left
/// undecided — a bound, so a peer that keeps handing out a page token cannot spin
/// this resolution without end. The same bound [`super::serverless`] reads its own
/// lookup listings under.
const MAX_LOOKUP_PAGES: usize = 16;

/// What asking the owning service about one zero-ETL source found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceResolution {
    /// That service holds the table or bucket the ARN names.
    Present,
    /// That service was asked and holds no such resource, so there is nothing for
    /// the integration to replicate. Which KIND of source was asked for travels
    /// with the answer, because the refusal names it.
    Absent(SourceKind),
    /// That service could not be asked, or the ARN names no resource kind this
    /// service knows how to resolve, so whether the source exists is UNKNOWN.
    /// That is this emulator failing to resolve a fact about itself, not a caller
    /// naming something that does not exist, so the two are never answered the
    /// same way. The reason travels for the operator-visible log.
    Unresolvable(String),
}

/// The kind of resource a zero-ETL `SourceArn` names, which is the service that
/// owns it: the two the modelled pattern admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An Amazon DynamoDB table.
    DynamoDbTable,
    /// An Amazon S3 bucket.
    S3Bucket,
}

impl SourceKind {
    /// How Amazon Redshift states that a source of this kind cannot be reached:
    /// it is not there, or the caller may not read it — one sentence for the two,
    /// because the service does not say which of them it is, and it points the
    /// caller at the policies on the source itself.
    ///
    /// Entirely FIXED text, carrying no value from the request: the condition is
    /// about the source the request already named. The DynamoDB sentence is the
    /// recorded one, word for word; the Amazon S3 sentence is the same sentence of
    /// the same error class with that service's own nouns, no recording of an
    /// S3-sourced create being available.
    pub fn inaccessible_message(self) -> &'static str {
        match self {
            Self::DynamoDbTable => {
                "You don't have access to zero-ETL integrations because the source DynamoDB \
                 table doesn't exist or you don't have the required permissions. Check the \
                 resource and key policy on the source table and make sure you have the \
                 required permissions. Refer to the Amazon DynamoDB Developer Guide for \
                 instructions on how to update the policy if required and then continue \
                 creating the integration."
            }
            Self::S3Bucket => {
                "You don't have access to zero-ETL integrations because the source Amazon S3 \
                 bucket doesn't exist or you don't have the required permissions. Check the \
                 resource and key policy on the source bucket and make sure you have the \
                 required permissions. Refer to the Amazon S3 User Guide for instructions on \
                 how to update the policy if required and then continue creating the \
                 integration."
            }
        }
    }
}

/// Resolve a zero-ETL integration's `SourceArn` against the service that owns the
/// resource it names.
///
/// Asked BEFORE any state of the TARGET is judged: a source that exists nowhere
/// can be replicated into no data warehouse, whatever state that warehouse is in.
/// Asked with no store lock held, like every other in-process hop.
pub async fn resolve_zero_etl_source(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    source_arn: &str,
) -> SourceResolution {
    let Some(arn) = ::service_common::arn::parse(source_arn) else {
        return SourceResolution::Unresolvable(
            "the source ARN does not carry the fields of a resource ARN".to_string(),
        );
    };
    // An ARN carries the Region its own resource is in; one that carries none —
    // as an Amazon S3 ARN does, S3 bucket names being partition-global — is
    // resolved in the Region the caller addressed. The field is read under the
    // shared Region-LABEL grammar every model's ARN pattern states for it
    // (`service_common::arn::region_label`), because this value ends up in the
    // AUTHORITY of the endpoint URL the in-process hop is addressed by: text
    // outside that grammar names no Region of any service, so the question is left
    // undecided rather than asked of an address the caller composed.
    let region = if arn.region.is_empty() {
        super::request_region(ctx)
    } else {
        match ::service_common::arn::region_label(arn.region) {
            Some(region) => region.to_string(),
            None => {
                return SourceResolution::Unresolvable(
                    "the source ARN's Region field is not a Region label".to_string(),
                )
            }
        }
    };
    match arn.vendor {
        DYNAMODB_ARN_VENDOR => {
            let Some(table) = ::service_common::arn::resource_id_of_type(
                source_arn,
                DYNAMODB_TABLE_RESOURCE_TYPE,
            ) else {
                return SourceResolution::Unresolvable(format!(
                    "the source ARN names a {} resource that is not a table",
                    DYNAMODB_ARN_VENDOR
                ));
            };
            table_resolution(ctx, &region, table).await
        }
        S3_ARN_VENDOR => {
            // The resource field of an Amazon S3 ARN is the bucket, optionally
            // followed by a key: the BUCKET is the resource that has to exist for
            // the integration to have anything to read.
            let bucket = arn.resource.split('/').next().unwrap_or(arn.resource);
            bucket_resolution(ctx, &region, bucket).await
        }
        other => SourceResolution::Unresolvable(format!(
            "no service of this emulator answers for a {other} zero-ETL source"
        )),
    }
}

/// Whether Amazon DynamoDB holds the table `name` in `region`.
///
/// Asked with `DescribeTable`, whose own `ResourceNotFoundException` ("the
/// operation tried to access a nonexistent table") is that service saying the
/// table is not there; any OTHER failure says nothing about whether it exists.
async fn table_resolution(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> SourceResolution {
    let answer = dynamodb_client(ctx, region)
        .describe_table()
        .table_name(name)
        .send()
        .await;
    match answer {
        Ok(_) => SourceResolution::Present,
        Err(error) => {
            if error.as_service_error().is_some_and(
                ::aws_sdk_dynamodb::operation::describe_table::DescribeTableError::is_resource_not_found_exception,
            ) {
                return SourceResolution::Absent(SourceKind::DynamoDbTable);
            }
            SourceResolution::Unresolvable(
                ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string(),
            )
        }
    }
}

/// Whether Amazon S3 holds the bucket `name`.
///
/// Asked with `ListBuckets`, which answers with the buckets the account holds:
/// an answer carrying no bucket of exactly that name is Amazon S3 saying the
/// bucket is not there. Asked that way rather than with `HeadBucket`, whose
/// refusal carries no body at all ("A message body isn't included, so you can't
/// determine the exception beyond these HTTP response codes" — that operation's
/// own documentation), so an emulator with no object store beside this one could
/// not be told apart from an account that really holds no such bucket. A listing
/// that cannot be read says nothing either way.
async fn bucket_resolution(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> SourceResolution {
    let client = ::service_common::s3_client::s3_client(ctx.callback(), region);
    let mut token: Option<String> = None;
    for _ in 0..MAX_LOOKUP_PAGES {
        let page = match client
            .list_buckets()
            .set_continuation_token(token)
            .send()
            .await
        {
            Ok(page) => page,
            Err(error) => {
                return SourceResolution::Unresolvable(
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string(),
                )
            }
        };
        if page
            .buckets()
            .iter()
            .any(|bucket| bucket.name() == Some(name))
        {
            return SourceResolution::Present;
        }
        token = page.continuation_token().map(str::to_string);
        if token.is_none() {
            return SourceResolution::Absent(SourceKind::S3Bucket);
        }
    }
    SourceResolution::Unresolvable(
        "the bucket listing handed out more pages than this lookup reads".to_string(),
    )
}

/// The Amazon DynamoDB client a table source is resolved through, configured
/// exactly as this crate's other in-process clients are and for the same reasons:
/// the callback routes IN PROCESS to this emulator's own inbound listener, so the
/// endpoint URL is addressing for that transport rather than a host anything
/// connects to; `region` is the Region the source's own ARN names; and the
/// credentials are the callback's, which sign this hop and never appear in a
/// response.
fn dynamodb_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_dynamodb::Client {
    let config = ::aws_sdk_dynamodb::Config::builder()
        .behavior_version(::aws_sdk_dynamodb::config::BehaviorVersion::latest())
        .region(::aws_sdk_dynamodb::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://dynamodb.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_dynamodb::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_dynamodb::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_dynamodb::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_dynamodb::Client::from_conf(config)
}
