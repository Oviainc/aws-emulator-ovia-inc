// SPDX-License-Identifier: CC0-1.0

//! What a request's REFERENCE to a resource of ANOTHER service names, which this service
//! settles before it stores a record that depends on it: the Lambda function a cloud
//! connector's endpoint invokes, and the IAM role a destination delivers under.
//!
//! A reference to a resource of THIS service is resolved against this service's own store
//! (`CreateConnectorDestination`'s `CloudConnectorId`, `CreateAccountAssociation`'s
//! `ConnectorDestinationId`), so it needs nothing from here. The two members below name
//! resources this service does not hold, and they are reached the way any emulated service
//! reaches another: the AWS SDK driven over the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN PROCESS — no
//! socket, no outbound request, no host resolution. A function a caller created through
//! Lambda's own `CreateFunction`, and a role it created through IAM's own `CreateRole`, are
//! therefore the function and the role these readings resolve against.
//!
//! WHAT GROUNDS A REFUSAL. A request is refused only where the value's own FORM names no
//! such resource at all, or where the other service ANSWERED that it holds none. Anything
//! else — an assembly with neither Lambda nor IAM mounted, an answer this emulator could
//! not read — establishes nothing about the reference, so it refuses nothing: a refusal has
//! to be grounded in what was answered, never assumed.

use crate::ops::types::EndpointConfig;

/// The IAM service principal managed integrations assumes a caller-supplied role as — the
/// `RoleArn` a destination is created with, which the service delivers notifications under.
///
/// AWS names a service principal `<the service's own namespace>.amazonaws.com`, and this
/// service's namespace is its `aws.api#service$arnNamespace`, `iotmanagedintegrations`
/// (`smithy_query(query="service_info")`) — which is also the signing name and the service
/// segment of every ARN it mints ([`super::SERVICE_ARN_NAMESPACE`]).
const SERVICE_PRINCIPAL: &str = "iotmanagedintegrations.amazonaws.com";

/// The ARN vendor (service namespace) a cloud connector's Lambda endpoint is resolved
/// against: the first label of the Lambda endpoint host, and the vendor field of every
/// function ARN.
const LAMBDA_VENDOR: &str = "lambda";

/// How `CreateCloudConnector` states that the `EndpointConfig` it was given names a Lambda
/// function that is not there.
///
/// FIXED text: the sentence names the member class and interpolates no request value, the
/// way the recorded refusal of a create whose endpoint named an absent function words it.
pub const LAMBDA_ENDPOINT_ABSENT_MESSAGE: &str = "Connector endpoint Lambda ARN does not exist";

/// How a destination call states that the `RoleArn` it was given is not a role this service
/// can deliver as — either because the value names no IAM role, or because IAM answered
/// that the role cannot be assumed.
///
/// FIXED text, for the same reason: the recorded refusal of a create whose `RoleArn` could
/// not be assumed states both halves of the fault in one sentence and quotes no value back.
pub const DELIVERY_ROLE_UNUSABLE_MESSAGE: &str =
    "Role ARN is invalid or cannot be assumed. Please check the role configuration and \
     permissions.";

/// Whether the function a cloud connector's `EndpointConfig` names is one Lambda ANSWERED
/// it does not hold.
///
/// A C2C connector's endpoint is how the connector reaches the cloud it connects to — a
/// `LAMBDA` endpoint INVOKES the function its `arn` names — so the function is resolved
/// when the connector is written, and a connector naming a function that is not there is
/// refused rather than stored.
///
/// `false` — do not refuse — for an `EndpointConfig` that names no function at all (there
/// is nothing to resolve), and for every answer that is NOT Lambda reporting the function
/// absent, including an assembly with no Lambda to ask: see the module docs.
pub async fn lambda_endpoint_is_absent(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    endpoint_config: &EndpointConfig,
) -> bool {
    let Some(arn) = endpoint_config
        .lambda
        .as_ref()
        .map(|lambda| lambda.arn.as_str())
    else {
        return false;
    };
    // `FunctionName` takes a function's full ARN, its bare name, or either with a version
    // or alias qualifier — exactly the set `LambdaArn`'s modelled pattern admits — so the
    // value goes over as the request carried it.
    let answer = lambda_client(ctx, region, arn)
        .get_function()
        .function_name(arn)
        .send()
        .await;
    match answer {
        Ok(_) => false,
        Err(error) => {
            if error.as_service_error().is_some_and(
                ::aws_sdk_lambda::operation::get_function::GetFunctionError::is_resource_not_found_exception,
            ) {
                return true;
            }
            // Any other answer is this emulator failing to ASK rather than Lambda
            // reporting an absent function, so it refuses nothing — and it is worth a
            // warning, because an assembly with no Lambda to ask is one where this
            // reading does not run. The caller's own value is rendered in its escaped
            // `Debug` form, so a value carrying a newline cannot forge a log line.
            ::tracing::warn!(
                lambda = ?arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a cloud connector's endpoint function could not be resolved against Lambda"
            );
            false
        }
    }
}

/// Whether the `RoleArn` a destination call carries is one this service cannot deliver as:
/// the value names no IAM role of this account, or IAM ANSWERED that the role cannot be
/// assumed.
///
/// A destination's `RoleArn` is `@arnReference(AWS::IAM::Role)`
/// (`smithy_query(query="shape",shape="DeliveryDestinationRoleArn")`) and the delivery is
/// performed AS that role, so the two halves of the question are:
///
/// * does the value name an IAM role THIS account holds — a value of another form names no
///   role at all, and one naming another account's role is a role no principal of this
///   account can hand to a service
///   (`service_common::iam_role::role_name`, the tree's one reading of a role ARN);
/// * and does IAM answer that the role can be assumed by this service
///   (`service_common::iam_role::role_is_unassumable`, the tree's one IAM reading), which
///   refuses only what IAM ANSWERED — so in an assembly with no IAM to ask, a role ARN of
///   this account is not refused.
///
/// Either way a refused call stores nothing and the record is left exactly as it was.
pub async fn delivery_role_is_unusable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    if ::service_common::iam_role::role_name(role_arn).is_none() {
        return true;
    }
    ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL).await
}

/// The Lambda client a connector endpoint's function is resolved through: the in-process
/// callback into this same emulator, at the endpoint of the function's OWN Region.
///
/// The Region comes off the ARN when the value carries one, since that is the endpoint the
/// function lives behind, and is read through `service_common`'s one ARN parse, which
/// reports it only for an ARN OF LAMBDA whose Region field is a Region label: the value
/// lands in the AUTHORITY of the endpoint URL, so a caller-supplied value outside that
/// grammar must not reach it. A bare function name carries no Region, so it resolves in the
/// Region the request was signed for — where this service's own records live.
fn lambda_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    lambda_arn: &str,
) -> ::aws_sdk_lambda::Client {
    let addressed = ::service_common::arn::region_of(lambda_arn, LAMBDA_VENDOR)
        .or_else(|| ::service_common::arn::region_label(region))
        .unwrap_or(super::DEFAULT_REGION);
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(addressed.to_string()))
        .endpoint_url(format!("https://lambda.{addressed}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}
