// SPDX-License-Identifier: CC0-1.0

//! @op CreateCloudFormationChangeSet
//! Creates an AWS CloudFormation change set that deploys one PUBLISHED version of an
//! application in [`crate::state`] onto a CloudFormation stack.
//!
//! The change set is not a record of this service's own: "This property corresponds to
//! the parameter of the same name for the AWS CloudFormation CreateChangeSet API" is what
//! the model says about nearly every member, and the `ChangeSetId` this call answers with
//! is the ARN a client then passes to CloudFormation's `DescribeChangeSet` /
//! `ExecuteChangeSet`. So the handler resolves the application and the version in THIS
//! service's store, reads the template that version published, and creates the change set
//! in CloudFormation over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — which is where the change set
//! then lives, exactly as it does in AWS. The stack it belongs to is
//! `serverlessrepo-<StackName>`, the name a deployment of a published application holds
//! its stack under, and the change set records WHICH application and WHICH version of it
//! was deployed as the stack's own tags ([`stack_tags`]), the provenance a client reads
//! back off a deployed stack. A deployment of a published application goes through the
//! serverless transform, so it acknowledges `CAPABILITY_IAM` to CloudFormation for every
//! deployment it makes ([`stack_capabilities`]), beside the acknowledgement the request
//! made to this service.
//!
//! An application this catalogue does not hold, a `SemanticVersion` it never published
//! and a `TemplateId` it never prepared are absent resources: HTTP 404
//! `NotFoundException`. An application that holds no version at all, a `Capabilities`
//! acknowledgement the version's template is not satisfied by, and every other request
//! outside the contract are the modelled 400 `BadRequestException`, and leave nothing
//! behind in CloudFormation.
//!
//! This is the only file to touch for this operation's own behaviour; the capability
//! requirement it enforces is derived and stored with a published version by
//! [`crate::state::required_capabilities_of`], so the two publish handlers and
//! `Version$RequiredCapabilities` cannot drift from it.
//!
//! The engine is held on `self` (constructor-injected); reach the store through
//! [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata and the
//! callback into this emulator.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCloudFormationChangeSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json")
//! ```
//!
//! Creates an AWS CloudFormation change set for the given application.
//!
//! ## Input
//!
//! * `ApplicationId` (__string) **required** — The Amazon Resource Name (ARN) of the
//!   application.
//! * `Capabilities` (__listOf__string) — A list of values that you must specify before you
//!   can deploy certain applications. Some applications might include resources that can
//!   affect permissions in your AWS account, for example, by creating new AWS Identity and
//!   Access Management (IAM) users. For those applications, you must explicitly acknowledge
//!   their capabilities by specifying this parameter. The only valid values are
//!   CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_RESOURCE_POLICY, and
//!   CAPABILITY_AUTO_EXPAND. The following resources require you to specify CAPABILITY_IAM or
//!   CAPABILITY_NAMED_IAM: AWS::IAM::Group, AWS::IAM::InstanceProfile, AWS::IAM::Policy, and
//!   AWS::IAM::Role. If the application contains IAM resources, you can …
//! * `ChangeSetName` (__string) — This property corresponds to the parameter of the same name
//!   for the AWS CloudFormation CreateChangeSet API.
//! * `ClientToken` (__string) — This property corresponds to the parameter of the same name
//!   for the AWS CloudFormation CreateChangeSet API.
//! * `Description` (__string) — This property corresponds to the parameter of the same name
//!   for the AWS CloudFormation CreateChangeSet API.
//! * `NotificationArns` (__listOf__string) — This property corresponds to the parameter of
//!   the same name for the AWS CloudFormation CreateChangeSet API.
//! * `ParameterOverrides` (__listOfParameterValue) — A list of parameter values for the
//!   parameters of the application.
//! * `ResourceTypes` (__listOf__string) — This property corresponds to the parameter of the
//!   same name for the AWS CloudFormation CreateChangeSet API.
//! * `RollbackConfiguration` (RollbackConfiguration) — This property corresponds to the
//!   parameter of the same name for the AWS CloudFormation CreateChangeSet API.
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `StackName` (__string) **required** — This property corresponds to the parameter of the
//!   same name for the AWS CloudFormation CreateChangeSet API.
//! * `Tags` (__listOfTag) — This property corresponds to the parameter of the same name for
//!   the AWS CloudFormation CreateChangeSet API.
//! * `TemplateId` (__string) — The UUID returned by CreateCloudFormationTemplate. Pattern:
//!   [0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}
//!
//! ## Output
//!
//! * `ApplicationId` (__string) — The application Amazon Resource Name (ARN).
//! * `ChangeSetId` (__string) — The Amazon Resource Name (ARN) of the change set. Length
//!   constraints: Minimum length of 1. Pattern: ARN:[-a-zA-Z0-9:/]*
//! * `SemanticVersion` (__string) — The semantic version of the application:
//!   https://semver.org/
//! * `StackId` (__string) — The unique ID of the stack.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — One of the parameters in the request is invalid.
//! * `ForbiddenException` (HTTP 403) — The client is not authenticated.
//! * `InternalServerErrorException` (HTTP 500) — The AWS Serverless Application Repository
//!   service encountered an internal error.
//! * `TooManyRequestsException` (HTTP 429) — The client is sending more than the allowed
//!   number of requests per unit of time.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateCloudFormationChangeSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateCloudFormationChangeSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
// `extra_errors` widens the set with `NotFoundException`, a shape this SERVICE models
// (and `CreateCloudFormationTemplate`/`GetApplication` list) that this operation's own
// `errors` list omits: the recorded behaviour for a `SemanticVersion` the application
// never published is HTTP 404 with error code `NotFoundException`, and an `ApplicationId`
// naming no application is as absent as that version.
#[smithy_macros::operation_error(
    "com.amazonaws.serverlessapplicationrepository#CreateCloudFormationChangeSet",
    extra_errors = ["NotFoundException"]
)]
#[derive(Debug)]
pub enum CreateCloudFormationChangeSetErrorModeled {
    BadRequestException(BadRequestException),
    ForbiddenException(ForbiddenException),
    InternalServerErrorException(InternalServerErrorException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_cloud_formation_change_set::CreateCloudFormationChangeSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateCloudFormationChangeSetError = crate::scaffolded::Scaffolded<CreateCloudFormationChangeSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateCloudFormationChangeSetErrorModeled> for CreateCloudFormationChangeSetError {
    fn from(modeled: CreateCloudFormationChangeSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateCloudFormationChangeSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCloudFormationChangeSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCloudFormationChangeSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCloudFormationChangeSet<Eng>
{
    type Input = CreateCloudFormationChangeSetInput;
    type Output = CreateCloudFormationChangeSetOutput;
    type Error = CreateCloudFormationChangeSetError;

    const NAME: &'static str = "CreateCloudFormationChangeSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_FORMATION_CHANGE_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLOUD_FORMATION_CHANGE_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_server_error("the service state is unavailable"));
        };
        let region = crate::state::request_region(ctx);

        // "The only valid values are CAPABILITY_IAM, CAPABILITY_NAMED_IAM,
        // CAPABILITY_RESOURCE_POLICY, and CAPABILITY_AUTO_EXPAND." The member targets
        // `__listOf__string`, so the boundary validator enforces no set and this does.
        let acknowledged = input.capabilities.clone().unwrap_or_default();
        if let Some(unknown) = acknowledged
            .iter()
            .find(|capability| !CAPABILITY_VALUES.contains(&capability.as_str()))
        {
            return Err(bad_request(format!(
                "Capabilities must each be one of {}: {unknown}",
                CAPABILITY_VALUES.join(", ")
            )));
        }
        // The version is named by its semantic version, so a value that is not one names
        // nothing: "The semantic version of the application: https://semver.org/". The
        // refusal is the service's fixed sentence for the version grammar
        // ([`crate::state::INVALID_SEMANTIC_VERSION_MESSAGE`]).
        if let Some(semantic_version) = &input.semantic_version {
            if !crate::state::is_semantic_version(semantic_version) {
                return Err(bad_request(crate::state::INVALID_SEMANTIC_VERSION_MESSAGE));
            }
        }
        // The application and the version this call deploys, read out of the store and
        // copied, so nothing is held locked across the call into CloudFormation.
        //
        // An application the catalogue does not hold, a `SemanticVersion` it never
        // published and a `TemplateId` it never prepared are ABSENT resources: HTTP 404
        // `NotFoundException`, the recorded answer for a version that was never published
        // and the one `GetCloudFormationTemplate` gives for a template of neither. An
        // application that holds no version at all is a different condition — the request
        // named none and there is none to choose — and its recorded answer is the
        // modelled 400.
        let now = ::std::time::SystemTime::now();
        let deployment = {
            let store = state.lock();
            let Some(application) = store.applications.get(&input.application_id) else {
                return Err(not_found(
                    crate::state::ErrorCode::ApplicationNotFound,
                    crate::state::application_not_found_message(&input.application_id),
                ));
            };
            match &input.template_id {
                // "The UUID returned by CreateCloudFormationTemplate": a template this
                // service PREPARED for this application, whose own `SemanticVersion` is
                // the version it was prepared from — so that is the version deployed.
                Some(template_id) => {
                    let Some(template) =
                        store.template_of(&input.application_id, template_id)
                    else {
                        return Err(not_found(
                            crate::state::ErrorCode::TemplateNotFound,
                            crate::state::template_not_found_message(template_id),
                        ));
                    };
                    // Only a prepared template can be deployed: one still `PREPARING` is
                    // not ready, and an expired one is gone ("Templates expire 1 hour
                    // after creation").
                    let status = template.status_at(now);
                    if status != crate::state::TEMPLATE_ACTIVE {
                        return Err(bad_request(format!(
                            "Template with id {template_id} is {status} and cannot be \
                             deployed."
                        )));
                    }
                    // Two members naming two different versions is one request asking for
                    // two deployments.
                    if let Some(requested) = &input.semantic_version {
                        if *requested != template.semantic_version {
                            return Err(bad_request(format!(
                                "TemplateId {template_id} was prepared from version {}, \
                                 which is not the SemanticVersion {requested} the request \
                                 names.",
                                template.semantic_version
                            )));
                        }
                    }
                    let prepared_from = template.semantic_version.clone();
                    application.version_named(&prepared_from).ok_or_else(|| {
                        not_found(
                            crate::state::ErrorCode::VersionNotFound,
                            // The ONE wording for this condition, shared with every other
                            // operation that resolves a version.
                            crate::state::version_not_found_message(
                                &input.application_id,
                                &prepared_from,
                            ),
                        )
                    })
                }
                None => match &input.semantic_version {
                    Some(requested) => application.version_named(requested).ok_or_else(|| {
                        not_found(
                            crate::state::ErrorCode::VersionNotFound,
                            crate::state::version_not_found_message(
                                &input.application_id,
                                requested,
                            ),
                        )
                    }),
                    // "The semantic version of the application" is optional, so a request
                    // that names none deploys the version the application reports as its
                    // latest — and an application with no version at all has nothing to
                    // deploy.
                    None => application
                        .latest()
                        .ok_or_else(|| bad_request(NO_VERSION_TO_DEPLOY)),
                },
            }
            .map(|version| Deployment {
                semantic_version: version.semantic_version.clone(),
                template_body: version.template_body.clone(),
                template_url: version.template_url.clone(),
                // The acknowledgement this version was published with, as
                // `Version$RequiredCapabilities` reports it.
                required_capabilities: version.required_capabilities.clone(),
            })
        }?;

        // The template is what gets deployed. A version published by `TemplateUrl` alone
        // holds no body, and this emulator makes no outbound connections, so there is
        // nothing to deploy and nothing is guessed at.
        let Some(template_body) = deployment.template_body.clone() else {
            return Err(crate::scaffolded::Scaffolded::not_implemented(&format!(
                "CreateCloudFormationChangeSet for a version whose template is only named \
                 by a URL ({}): fetching it would be an outbound request",
                deployment.template_url
            )));
        };
        // "For those applications, you must explicitly acknowledge their capabilities by
        // specifying this parameter. … If you don't specify this parameter for an
        // application that requires capabilities, the call will fail."
        let missing = crate::state::unacknowledged_capabilities(
            &deployment.required_capabilities,
            &acknowledged,
        );
        if !missing.is_empty() {
            return Err(bad_request(format!(
                "Required capabilities [{}] were not provided.",
                missing.join(", ")
            )));
        }

        // The stack a deployment of a published application goes into is
        // `serverlessrepo-<StackName>`: this service owns the stacks it deploys, and the
        // prefix is what the recorded deployments show CloudFormation holding them under.
        let stack_name = format!("{STACK_NAME_PREFIX}{}", input.stack_name);
        let cloudformation = cloudformation_client(ctx, &region);
        // CloudFormation's own `ChangeSetType`: "You can't use the UPDATE type to create a
        // change set for a new stack or the CREATE type to create a change set for an
        // existing stack." A stack still in `REVIEW_IN_PROGRESS` is one a CREATE change
        // set created and nothing has executed, so it does not exist yet either.
        let change_set_type = match live_stack_status(&cloudformation, &stack_name).await? {
            Some(status) if status != REVIEW_IN_PROGRESS => {
                ::aws_sdk_cloudformation::types::ChangeSetType::Update
            }
            _ => ::aws_sdk_cloudformation::types::ChangeSetType::Create,
        };

        let created = cloudformation
            .create_change_set()
            .stack_name(&stack_name)
            // "This property corresponds to the parameter of the same name for the AWS
            // CloudFormation CreateChangeSet API" — which requires one, so a request that
            // names none gets the name this service mints for it.
            .change_set_name(
                input
                    .change_set_name
                    .clone()
                    .unwrap_or_else(new_change_set_name),
            )
            .change_set_type(change_set_type)
            .template_body(template_body)
            .set_description(input.description.clone())
            .set_client_token(input.client_token.clone())
            .set_capabilities(Some(stack_capabilities(&acknowledged)))
            .set_notification_arns(input.notification_arns.clone())
            .set_resource_types(input.resource_types.clone())
            // "A list of parameter values for the parameters of the application" — the
            // stack parameters the deployment is made with. A request that carries none
            // passes none, so a deployment onto a stack that already exists keeps that
            // stack's previous parameter values rather than clearing them.
            .set_parameters(input.parameter_overrides.as_deref().map(stack_parameters))
            // The deployment's own provenance tags, then the request's own; see
            // [`stack_tags`].
            .set_tags(Some(stack_tags(
                &input.application_id,
                &deployment.semantic_version,
                input.tags.as_deref().unwrap_or_default(),
            )))
            .set_rollback_configuration(rollback_configuration(
                input.rollback_configuration.as_ref(),
            ))
            .send()
            .await
            .map_err(cloudformation_refusal)?;

        // Answered from what CloudFormation created: the change set's ARN and the stack it
        // belongs to are CloudFormation's to mint, and the application and version are the
        // ones this call resolved in the store.
        Ok(CreateCloudFormationChangeSetOutput {
            application_id: Some(input.application_id.clone()),
            change_set_id: created.id().map(str::to_string),
            semantic_version: Some(deployment.semantic_version),
            stack_id: created.stack_id().map(str::to_string),
        })
    }
}

/// The published version this call deploys, copied out of the store.
struct Deployment {
    semantic_version: String,
    /// The template as published inline, which is the body the change set is created
    /// from. `None` for a version published by URL alone.
    template_body: Option<String>,
    /// The link the version reports for its template, named in the refusal for a version
    /// whose body this emulator does not hold.
    template_url: String,
    /// What a deployment of this version must acknowledge, as the version was published
    /// with it; see [`crate::state::required_capabilities_of`].
    required_capabilities: Vec<String>,
}

/// The status of the stack `stack_name` names, or `None` when CloudFormation holds no
/// such stack.
///
/// "If the stack doesn't exist, a ValidationError is returned" — so that one code is the
/// absence. Every other refusal is classified exactly as the create's own is
/// ([`throttled`]): the caller calling too fast is the modelled 429 whichever of the two
/// calls met it, and anything else is this emulator failing to ask, which is a server
/// error rather than a stack that is not there.
async fn live_stack_status(
    cloudformation: &::aws_sdk_cloudformation::Client,
    stack_name: &str,
) -> Result<Option<String>, CreateCloudFormationChangeSetError> {
    match cloudformation
        .describe_stacks()
        .stack_name(stack_name)
        .send()
        .await
    {
        Ok(answer) => Ok(answer
            .stacks()
            .first()
            .and_then(|stack| stack.stack_status())
            .map(|status| status.as_str().to_string())),
        Err(error) => match ::aws_sdk_cloudformation::error::ProvideErrorMetadata::code(&error) {
            Some(STACK_DOES_NOT_EXIST_CODE) => Ok(None),
            Some(code) if throttled(code) => Err(too_many_requests(
                ::aws_sdk_cloudformation::error::ProvideErrorMetadata::message(&error)
                    .unwrap_or("retry the request"),
            )),
            _ => Err(internal_server_error(format!(
                "AWS CloudFormation could not be asked about stack {stack_name}: {}",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            ))),
        },
    }
}

/// The error code CloudFormation answers for a stack it does not hold.
const STACK_DOES_NOT_EXIST_CODE: &str = "ValidationError";

/// The stack status CloudFormation gives a stack a CREATE change set brought into
/// existence and nothing has executed yet: "the stack will be in the REVIEW_IN_PROGRESS
/// state until you execute the change set".
const REVIEW_IN_PROGRESS: &str = "REVIEW_IN_PROGRESS";

/// What this service prefixes a deployment's `StackName` with in CloudFormation; see the
/// call site.
const STACK_NAME_PREFIX: &str = "serverlessrepo-";

/// The 400 this operation answers for an application that has published no version at all.
///
/// The sentence is FIXED: it names neither the application nor a version, because there is
/// no version to name and this condition is about the operation that cannot be served, not
/// about a value in the request. It belongs to THIS deploy path alone — the sibling
/// `CreateCloudFormationTemplate` states the same precondition in its own words — so it is
/// stated here rather than in a helper the two would share.
const NO_VERSION_TO_DEPLOY: &str =
    "Can't create cloudformation changeset from application with no version.";

/// A CloudFormation client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and this service names
/// no other service's crate. `region` is this request's own signing region, so the change
/// set and its stack land in the region the caller asked for.
fn cloudformation_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudformation::Client {
    let config = ::aws_sdk_cloudformation::Config::builder()
        .behavior_version(::aws_sdk_cloudformation::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudformation::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://cloudformation.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_cloudformation::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudformation::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudformation::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudformation::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudformation::Client::from_conf(config)
}

/// The capabilities the change set is created with in CloudFormation.
///
/// A deployment of a published application is made through the serverless transform, which
/// synthesizes the IAM roles the application's functions run under, so this service
/// acknowledges `CAPABILITY_IAM` to CloudFormation for EVERY deployment it makes —
/// whatever the request acknowledged to it, and whatever the template declares, including
/// one whose resources require no acknowledgement at all, which is what a deployment of a
/// queue-only application is observed to carry. Which acknowledgement the CALLER owes is a
/// separate question, answered before this from the version's own requirement
/// ([`crate::state::required_capabilities_of`]); this is what the service acknowledges on
/// the deployment's behalf once that holds. The request's own values are passed on beside
/// it, in the order the request named them and without repeating a value.
///
/// `CAPABILITY_RESOURCE_POLICY` is one of the four values THIS service's `Capabilities`
/// documents, but CloudFormation's own `Capability` enum admits only
/// `CAPABILITY_IAM | CAPABILITY_NAMED_IAM | CAPABILITY_AUTO_EXPAND`
/// (`smithy_query(query="shape", shape="Capability")` on the CloudFormation model), and a
/// value outside an enum's declared set is refused at CloudFormation's boundary. So the
/// resource-policy acknowledgement is this service's own — it is what lets the deployment
/// proceed here — and only the values CloudFormation models are passed on.
fn stack_capabilities(acknowledged: &[String]) -> Vec<::aws_sdk_cloudformation::types::Capability> {
    let mut capabilities = vec![::aws_sdk_cloudformation::types::Capability::CapabilityIam];
    for capability in acknowledged
        .iter()
        .filter(|capability| capability.as_str() != crate::state::CAPABILITY_RESOURCE_POLICY)
        .map(|capability| ::aws_sdk_cloudformation::types::Capability::from(capability.as_str()))
    {
        if !capabilities.contains(&capability) {
            capabilities.push(capability);
        }
    }
    capabilities
}

/// The request's `ParameterOverrides` as CloudFormation's own stack parameters.
fn stack_parameters(
    overrides: &[ParameterValue],
) -> Vec<::aws_sdk_cloudformation::types::Parameter> {
    overrides
        .iter()
        .map(|override_| {
            ::aws_sdk_cloudformation::types::Parameter::builder()
                .parameter_key(override_.name.clone())
                .parameter_value(override_.value.clone())
                .build()
        })
        .collect()
}

/// The tags the change set, and so the stack it is executed onto, is created with: the two
/// tags this service records its own deployment under, then the request's `Tags`, which is
/// what that member means ("This property corresponds to the parameter of the same name
/// for the AWS CloudFormation CreateChangeSet API").
///
/// A deployment of a published application carries which application, and which version of
/// it, the stack was deployed from: that provenance is this service's own record on the
/// stack, so the two keys below are reserved — a request that names one of them does not
/// displace the deployment's own value.
fn stack_tags(
    application_id: &str,
    semantic_version: &str,
    requested: &[Tag],
) -> Vec<::aws_sdk_cloudformation::types::Tag> {
    let mut tags = vec![
        tag(APPLICATION_ID_TAG, application_id),
        tag(SEMANTIC_VERSION_TAG, semantic_version),
    ];
    tags.extend(
        requested
            .iter()
            .filter(|requested| {
                requested.key != APPLICATION_ID_TAG && requested.key != SEMANTIC_VERSION_TAG
            })
            .map(|requested| tag(&requested.key, &requested.value)),
    );
    tags
}

/// One CloudFormation stack tag.
fn tag(key: &str, value: &str) -> ::aws_sdk_cloudformation::types::Tag {
    ::aws_sdk_cloudformation::types::Tag::builder()
        .key(key)
        .value(value)
        .build()
}

/// The tag key a deployment records the deployed application's ARN under.
const APPLICATION_ID_TAG: &str = "serverlessrepo:applicationId";

/// The tag key a deployment records the deployed version under.
const SEMANTIC_VERSION_TAG: &str = "serverlessrepo:semanticVersion";

/// The request's `RollbackConfiguration` as CloudFormation's own.
fn rollback_configuration(
    configuration: Option<&RollbackConfiguration>,
) -> Option<::aws_sdk_cloudformation::types::RollbackConfiguration> {
    let configuration = configuration?;
    let triggers = configuration
        .rollback_triggers
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|trigger| {
            ::aws_sdk_cloudformation::types::RollbackTrigger::builder()
                .arn(trigger.arn)
                .r#type(trigger.r#type)
                .build()
        })
        .collect();
    Some(
        ::aws_sdk_cloudformation::types::RollbackConfiguration::builder()
            .set_monitoring_time_in_minutes(configuration.monitoring_time_in_minutes)
            .set_rollback_triggers(Some(triggers))
            .build(),
    )
}

/// What this operation answers when CloudFormation would not create the change set.
///
/// CloudFormation answering an error code means it JUDGED the request — the stack name,
/// the change set name, the template, a quota — so the caller's request is the invalid
/// one and this is the modelled 400, carrying what CloudFormation said. Two kinds of code
/// are NOT that: the ones that say CloudFormation could not serve the request (this
/// emulator's own failure, 500, as is no code at all — the call never got an answer), and
/// the ones that say the caller is calling too fast, which this operation models as
/// `TooManyRequestsException` (429).
fn cloudformation_refusal<E, R>(
    error: ::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> CreateCloudFormationChangeSetError
where
    ::aws_smithy_runtime_api::client::result::SdkError<E, R>:
        ::aws_sdk_cloudformation::error::ProvideErrorMetadata + ::std::fmt::Debug,
{
    let code = ::aws_sdk_cloudformation::error::ProvideErrorMetadata::code(&error);
    let message = ::aws_sdk_cloudformation::error::ProvideErrorMetadata::message(&error);
    match code {
        None | Some("InternalFailure") | Some("ServiceUnavailable") | Some("NotImplemented") => {
            internal_server_error(format!(
                "the change set could not be created in AWS CloudFormation: {}",
                message.unwrap_or("CloudFormation did not answer")
            ))
        }
        // The request was not wrong, it was too soon.
        Some(code) if throttled(code) => {
            too_many_requests(message.unwrap_or("retry the request"))
        }
        Some(code) => bad_request(format!(
            "AWS CloudFormation refused the change set: {code}: {}",
            message.unwrap_or("no further detail")
        )),
    }
}

/// Whether a CloudFormation error code says the caller is calling too fast rather than
/// that the request is wrong: the AWS throttling codes.
fn throttled(code: &str) -> bool {
    matches!(
        code,
        "Throttling" | "ThrottlingException" | "RequestLimitExceeded"
    )
}

/// The 429 this operation answers when AWS CloudFormation throttled one of the calls this
/// deployment makes to it.
fn too_many_requests(detail: &str) -> CreateCloudFormationChangeSetError {
    CreateCloudFormationChangeSetErrorModeled::TooManyRequestsException(TooManyRequestsException {
        error_code: Some(TOO_MANY_REQUESTS_ERROR_CODE.to_string()),
        message: Some(format!(
            "AWS CloudFormation is throttling this deployment: {detail}"
        )),
    })
    .into()
}

/// The condition a 429 names in its `errorCode`, in the `<Subject><Condition>Exception`
/// form every other condition in this service takes (see [`crate::state::ErrorCode`]).
const TOO_MANY_REQUESTS_ERROR_CODE: &str = "TooManyRequestsException";

/// The name this service mints for a change set the caller named none for.
///
/// CloudFormation requires a `ChangeSetName` and constrains it to
/// `[a-zA-Z][-a-zA-Z0-9]*`, which a uuid satisfies once it is prefixed with a name — so
/// this is the crate's own single id minter ([`crate::state::new_uuid`]) under a prefix
/// that says what minted it, and it is unique among the change sets of one stack because
/// that minter is.
fn new_change_set_name() -> String {
    format!("serverlessrepo-changeset-{}", crate::state::new_uuid())
}

/// The 400 this operation answers for a request outside the contract.
fn bad_request(message: impl Into<String>) -> CreateCloudFormationChangeSetError {
    CreateCloudFormationChangeSetErrorModeled::BadRequestException(crate::state::bad_request(
        message,
    ))
    .into()
}

/// The 500 this operation answers when it could not serve the request at all.
fn internal_server_error(message: impl Into<String>) -> CreateCloudFormationChangeSetError {
    CreateCloudFormationChangeSetErrorModeled::InternalServerErrorException(
        crate::state::internal_server_error(message),
    )
    .into()
}

/// The 404 this operation answers for an application, or a version of one, that this
/// catalogue does not hold.
fn not_found(
    code: crate::state::ErrorCode,
    message: String,
) -> CreateCloudFormationChangeSetError {
    CreateCloudFormationChangeSetErrorModeled::NotFoundException(crate::state::not_found(
        code, message,
    ))
    .into()
}
