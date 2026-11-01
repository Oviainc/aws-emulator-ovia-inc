// SPDX-License-Identifier: CC0-1.0

//! A `Task` state's service integration: which call its `Resource` names, and how
//! this emulator MAKES that call.
//!
//! A `Task` state "represents a single unit of work performed by a state machine",
//! and for a service integration that unit of work is one API call to another
//! service: the state's effective input — the document its `Parameters` payload
//! template builds — IS that operation's request, and the operation's response
//! document IS the state's result. That is the rule this module serves, and it is
//! served for real: the call goes to the other service's own handler in this same
//! emulator, over the in-process callback every emulated service reaches another by
//! ([`smithy_server_runtime::OperationContext::callback`]), signed with the
//! emulator's own credentials because the emulator verifies every request
//! signature. So what a `dynamodb:putItem` task writes is readable afterwards
//! through `GetItem`, and what the task reports is what DynamoDB answered — nothing
//! here synthesises a result.
//!
//! ## Which integrations are made
//!
//! A `Resource` of the form `arn:aws:states:::<namespace>:<action>` (an optimized
//! integration) or `arn:aws:states:::aws-sdk:<sdk name>:<action>` (an AWS SDK
//! integration) names a service — by its ARN namespace in the first family, by the
//! name the AWS SDKs give it in the second ([`Api::sdk_token`]) — and one of that
//! service's API operations by its name in lowerCamel. The call is made when that
//! service has an
//! [`APIS`] entry — the services whose protocol carries the request as ONE JSON
//! document, the `awsJson1_0` and `awsJson1_1` protocols, whose body IS the
//! operation's input document, which is exactly what a payload template builds. One
//! entry per service, and EVERY operation of a listed service is reachable through
//! it: the operation name is derived from the action, so nothing here is written per
//! action.
//!
//! The parameters of a `Task` state's integration are written in the integration's
//! own convention, PascalCase, whatever the invoked API spells its own members in,
//! so the document the state built is put on the wire under the spelling that
//! service's model uses ([`MemberCase`]) — and a member the integration documents as
//! a document while the operation types it as a string is carried as that document's
//! JSON text ([`JSON_TEXT_MEMBERS`]). The answer is reported back to the state in
//! that same convention ([`task_result`]). Both re-spell the TOP level only, because
//! below it a document carries a caller's DATA beside the operation's members: a
//! structure member nested inside a response therefore still reads as the invoked
//! model spells it, which is a divergence this emulator carries rather than one it
//! hides — re-spelling further needs the invoked model's own member names, which
//! this crate cannot read.
//!
//! What has no entry is not faked. Each of these is reported by [`read`] and ends the
//! execution in the service's own failure state with the ASL-documented
//! [`super::asl::RUNTIME_ERROR`] and a cause saying what was not run:
//!
//! * a service with no entry — one whose protocol binds its request to a shape the
//!   state's document is not (`awsQuery`'s form fields, a REST method and path with
//!   the payload at one member), since handing such a service a JSON document would
//!   not be its request, and equally one whose wire facts simply are not listed here
//!   yet;
//! * the `.waitForTaskToken` integration pattern, which needs a later
//!   `SendTaskSuccess` from outside the execution;
//! * a run-a-job (`.sync`) pattern over an integration that is not in the
//!   [`RUN_A_JOB`] catalogue below, because the job it starts is reported and
//!   stopped through calls only that integration names;
//! * a bare resource ARN (a Lambda function ARN as the `Resource`), which names a
//!   resource rather than an integration call.
//!
//! ## The run-a-job (`.sync`) pattern
//!
//! A `Resource` ending in `.sync` (or `.sync:2`) asks for the run-a-job pattern:
//! the integration's own call STARTS a job, and the task is not finished until that
//! job is. [`RUN_A_JOB`] carries, per integration, the three calls that pattern is
//! made of — the one that starts the job, the one that reports it and the one that
//! stops it — plus where the job's identifier, its status and the reason it failed
//! sit in those documents, which members of the STARTING request those two calls
//! carry beside the identifier for an integration whose job the identifier does not
//! name on its own, and the shape that stopping call types the identifier as.
//! The walk makes the starting call, records what came back
//! as the task's submission and PARKS there ([`super::asl::ParkedJob`]); the job is
//! then polled through its own reporting call until it reaches a terminal status,
//! which is what resolves the task ([`super::run_a_job`]). Nothing is synthesised:
//! the identifier, the status and the failure are the invoked service's own answers.
//!
//! An ACTIVITY `Resource` is not one of these: its work belongs to a worker that
//! polls this emulator, so the walk schedules a task and suspends at it rather than
//! asking for a call ([`super::asl::Interpreter`] decides that before [`read`] is
//! asked anything). [`read`] still answers for one, so it is total over every
//! `Resource` form a definition can carry.
//!
//! ## What a task may do, and what its call answers
//!
//! The call is made AS the state machine's execution role, so the policies in force
//! on that role decide whether it is made at all: the `(action, resource)` pair the
//! call is subject to is formed from the call itself ([`access_request`], over
//! [`TARGET_RESOURCES`]) and decided by [`super::task_authorization`]. A call that
//! role may not make is refused BEFORE the request goes anywhere, so nothing is
//! started for it, and the task fails with the invoked service's own access-denied
//! refusal.
//!
//! What a request-response call answers is reported to the state as the literal SDK
//! response an OPTIMIZED integration returns: the operation's own members under the
//! integration's convention, beside the SDK's response envelope
//! ([`with_sdk_envelope`]), whose status, headers and request id are that answer's
//! own. The AWS SDK family reports the operation's members alone, and the `.sync`
//! pattern reports the job's own report instead.
//!
//! ## Where the call goes
//!
//! Nowhere outside this process. The destination is never a value a caller
//! supplied: the `Resource`'s service token selects one of the [`APIS`] entries —
//! services this emulator hosts itself — and the request is handed to the
//! in-process callback, which is the emulator's own router rather than a socket.
//! No client is built, no address is resolved and no host is dialled; the host
//! header exists because the signature is computed over it. The Region is the
//! execution's own, from its state machine's ARN, so a definition cannot steer the
//! call anywhere either.
//!
//! ## What bounds it
//!
//! One interpretation runs on ONE blocking thread and HOLDS that thread while a
//! call it asked for is in flight, so the resource to bound is not only the work but
//! the threads a chain of calls can pin at once. Four bounds do it:
//!
//! * the walk charges every state transition to
//!   [`super::asl::MAX_STATE_TRANSITIONS`], which bounds how many calls ONE
//!   execution makes;
//! * [`MAX_CALL_DURATION`] bounds ONE call, server-side, on the thread that is
//!   waiting: a call that has not answered inside it fails the task instead of
//!   pinning the thread, which is also what clamps a target that holds its answer on
//!   purpose (`GetActivityTask` is a long poll);
//! * [`super::asl::MAX_INTERPRET_DURATION`] bounds the whole interpretation, and the
//!   walk now consults it BEFORE each call as well as at each transition, so a run
//!   that is past its deadline makes no further call;
//! * [`MAX_CALL_CHAIN_DEPTH`] bounds how deep a chain of in-process calls may still
//!   make one, which is what bounds the PRODUCT: a chain that re-enters this service
//!   — a task that starts another execution, or one that publishes an event a rule
//!   delivers to a state machine — holds one blocking thread per level, so the
//!   threads one inbound request can hold is that depth rather than the framework's
//!   own [`smithy_server_runtime::CALLBACK_RECURSION_LIMIT`], which is a cycle
//!   backstop set far above any legitimate call graph.
//!
//! A call refused by either ceiling is reported as the failure it is: nothing was
//! run for the task and nothing is faked. A call that TIMED OUT is the one case
//! where the task's answer and the world can differ: the wait is abandoned, but the
//! call it was waiting for is already with the other service, so a task that reports
//! `States.Timeout` may still have had its effect — which is exactly what a timed-out
//! call means on the real service too, and why its `Catch` path must not assume
//! otherwise.

use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};

/// How deep a chain of in-process calls may be and still make one.
///
/// Emulator bound, NOT an AWS limit. Every interpretation holds one blocking thread
/// for its whole duration, including while a call it made is in flight, so a chain
/// that re-enters this service occupies one thread per level: a task that starts
/// another execution, and equally one that publishes an event whose rule delivers to
/// a state machine. The framework's own
/// [`smithy_server_runtime::CALLBACK_RECURSION_LIMIT`] bounds such a chain at a
/// depth chosen as a cycle backstop, far above any legitimate call graph, which is
/// the wrong order of magnitude for a resource that is HELD rather than merely
/// spent. This is the depth at which a task's call is still made, so the blocking
/// threads one inbound request can pin is this many. A task deeper than it fails
/// with the ASL-documented [`super::asl::RUNTIME_ERROR`]: nothing was run for it,
/// and nothing is faked.
///
/// Three levels is above every chain this emulator serves — a published event
/// starting an execution whose task writes to another service is two — and far below
/// the blocking pool a request's chain must not exhaust.
pub const MAX_CALL_CHAIN_DEPTH: u8 = 3;

/// The longest the walk waits for ONE integration call to answer.
///
/// Emulator bound: the call is served in THIS process, so a real one answers in
/// microseconds, and the walk's own deadline cannot bound the time spent inside a
/// call because the walk is blocked there rather than transitioning. A call that has
/// not answered inside this fails the task with ASL's own timeout error instead of
/// holding the blocking thread the walk runs on, which also clamps a target that
/// holds its answer on purpose: `GetActivityTask` is a long poll, and this is what
/// stops one from pinning a thread for its hold.
///
/// It is the interpretation's own deadline, because a call that outlasts the time
/// this emulator interprets a whole definition in has already lost its run.
pub const MAX_CALL_DURATION: Duration = super::asl::MAX_INTERPRET_DURATION;

/// The ARN namespace every service-integration `Resource` is written under: the
/// `Resource` names Step Functions' own integration, not the target resource.
const STATES_NAMESPACE: &str = "states";

/// The `Resource` segment that selects the AWS SDK integration family
/// (`arn:aws:states:::aws-sdk:<service>:<action>`).
const AWS_SDK_SEGMENT: &str = "aws-sdk";

/// The wire error code an emulated service answers for an operation nobody has
/// implemented yet (`crate::scaffolded`). It is this emulator's own gap rather than
/// the service refusing the call, so a task that meets it reports that nothing ran.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The ASL predefined error name for a task whose own work failed
/// ("States.TaskFailed"), which is what a refusal carrying no error code at all
/// leaves.
pub const TASK_FAILED_ERROR: &str = "States.TaskFailed";

/// The most bytes the `resourceType` and `resource` a `Task` state's events report
/// carry: both target the modelled `Name` shape, whose `length` is 1..80
/// (`smithy_query(query="shape",shape="TaskScheduledEventDetails")`).
const MAX_NAME_BYTES: usize = 80;

/// The header an AWS JSON service names its error shape in, beside the `__type`
/// member of the body.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The header an AWS JSON client names the operation it calls in
/// (`<target prefix>.<OperationName>`).
const TARGET_HEADER: &str = "x-amz-target";

/// How one service's API is reached on the wire: the facts of that service's own
/// model that a request to it is built from.
///
/// Every field is read from the target service's Smithy model with
/// `smithy_query(query="service_info")`, so this table states no wire detail of its
/// own.
#[derive(Debug)]
pub struct Api {
    /// The ARN namespace a `Resource` names this service by (`awsService.arnNamespace`).
    pub namespace: &'static str,
    /// Its SigV4 signing name (`sigv4Name`) — also the credential scope that routes
    /// the call to this service inside this emulator.
    pub signing_name: &'static str,
    /// The leading label of its endpoint host (`awsService.endpointPrefix`).
    pub endpoint_prefix: &'static str,
    /// The `X-Amz-Target` prefix a client names an operation of this service under:
    /// the service shape's own name, as the model spells it.
    pub target_prefix: &'static str,
    /// The AWS JSON protocol version its model declares (`awsJson1_0` -> `1.0`),
    /// which names the media type of the request body.
    pub json_version: &'static str,
    /// Its SDK id (`awsService.sdkId`), which names the service half of a task
    /// error ([`Failed::error`]) and — lower-cased — the token an AWS SDK
    /// integration's `Resource` names this service by ([`Api::sdk_token`]).
    pub sdk_id: &'static str,
    /// How its model spells the members of an operation's input
    /// ([`MemberCase`]), which is what decides the spelling the document a `Task`
    /// state built goes on the wire under.
    pub member_case: MemberCase,
}

impl Api {
    /// The host one of this service's regional endpoints answers on.
    fn host(&self, region: &str) -> String {
        format!("{}.{region}.amazonaws.com", self.endpoint_prefix)
    }

    /// The media type this service's protocol version names.
    fn content_type(&self) -> String {
        format!("application/x-amz-json-{}", self.json_version)
    }

    /// The token an AWS SDK integration's `Resource`
    /// (`arn:aws:states:::aws-sdk:<token>:<action>`) names this service by: the
    /// name the AWS SDKs give the service, which is its SDK id lower-cased with
    /// its spaces dropped (`SFN` -> `sfn`, `EventBridge` -> `eventbridge`).
    ///
    /// That is a different token from the ARN namespace the OPTIMIZED family names
    /// the same service by (`states:startExecution` beside
    /// `aws-sdk:sfn:startExecution`), so each family is looked up under its own.
    fn sdk_token(&self) -> String {
        self.sdk_id
            .chars()
            .filter(|character| !character.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }
}

/// How one service's model spells the members of an operation's input.
///
/// A `Task` state's integration parameters are written in PascalCase whatever the
/// invoked API spells its own members in — `StateMachineArn` for a nested execution
/// whose API member is `stateMachineArn` — so the document the state built is put on
/// the wire under the spelling that service's model uses. The two conventions differ
/// in the FIRST character and nothing else, which is the whole of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberCase {
    /// `stateMachineArn`, `taskDefinition`: the first character is lower case.
    LowerCamel,
    /// `TableName`, `QueueUrl`: the first character is upper case.
    Pascal,
}

impl MemberCase {
    /// `member` under this convention's spelling of it.
    fn spell(self, member: &str) -> String {
        let mut characters = member.chars();
        let Some(first) = characters.next() else {
            return String::new();
        };
        match self {
            Self::LowerCamel => first.to_lowercase().chain(characters).collect(),
            Self::Pascal => first.to_uppercase().chain(characters).collect(),
        }
    }
}

/// The services a `Task` state's integration call is made to: those whose protocol
/// carries the request as one JSON document, so the document the state built IS the
/// operation's input.
///
/// One entry per SERVICE, never per action: the action of a `Resource` names the
/// operation, so every operation each of these services models is reachable.
///
/// It is a LIST rather than a rule because each entry's wire facts are that
/// service's own model's, which this crate cannot read; a service whose model is
/// mounted here but has no entry yet is one no call is made for, and [`read`] says
/// exactly that rather than claiming anything about its protocol.
pub const APIS: &[Api] = &[
    // `services/service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json`: awsJson1_1, sigv4Name `athena`,
    // endpointPrefix `athena`, sdkId `Athena`, service shape `AmazonAthena`.
    Api {
        namespace: "athena",
        signing_name: "athena",
        endpoint_prefix: "athena",
        target_prefix: "AmazonAthena",
        json_version: "1.1",
        sdk_id: "Athena",
        // `StartQueryExecution`'s input: `QueryString`, `WorkGroup`.
        member_case: MemberCase::Pascal,
    },
    // `services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json`: awsJson1_0, sigv4Name
    // `dynamodb`, endpointPrefix `dynamodb`, sdkId `DynamoDB`, service shape
    // `DynamoDB_20120810`.
    Api {
        namespace: "dynamodb",
        signing_name: "dynamodb",
        endpoint_prefix: "dynamodb",
        target_prefix: "DynamoDB_20120810",
        json_version: "1.0",
        sdk_id: "DynamoDB",
        // `PutItem`'s input: `TableName`, `Item`.
        member_case: MemberCase::Pascal,
    },
    // `services/service-com-amazonaws-sqs-amazonsqs/smithy-model/
    // com-amazonaws-sqs-amazonsqs.json`: awsJson1_0, sigv4Name `sqs`,
    // endpointPrefix `sqs`, sdkId `SQS`, service shape `AmazonSQS`.
    Api {
        namespace: "sqs",
        signing_name: "sqs",
        endpoint_prefix: "sqs",
        target_prefix: "AmazonSQS",
        json_version: "1.0",
        sdk_id: "SQS",
        // `SendMessage`'s input: `QueueUrl`, `MessageBody`.
        member_case: MemberCase::Pascal,
    },
    // `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/
    // com-amazonaws-eventbridge-awsevents.json`: awsJson1_1, sigv4Name `events`,
    // endpointPrefix `events`, sdkId `EventBridge`, service shape `AWSEvents`.
    Api {
        namespace: "events",
        signing_name: "events",
        endpoint_prefix: "events",
        target_prefix: "AWSEvents",
        json_version: "1.1",
        sdk_id: "EventBridge",
        // `PutEvents`' input: `Entries`, `EndpointId`.
        member_case: MemberCase::Pascal,
    },
    // This service itself, for a task that starts another execution:
    // `services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/
    // com-amazonaws-sfn-awsstepfunctions.json`: awsJson1_0, sigv4Name `states`,
    // endpointPrefix `states`, sdkId `SFN`, service shape `AWSStepFunctions`.
    Api {
        namespace: STATES_NAMESPACE,
        signing_name: STATES_NAMESPACE,
        endpoint_prefix: STATES_NAMESPACE,
        target_prefix: "AWSStepFunctions",
        json_version: "1.0",
        sdk_id: "SFN",
        // `StartExecution`'s input: `stateMachineArn`, `name`, `input`.
        member_case: MemberCase::LowerCamel,
    },
    // `services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/
    // com-amazonaws-kinesis-kinesis-20131202.json`: awsJson1_1, sigv4Name
    // `kinesis`, endpointPrefix `kinesis`, sdkId `Kinesis`, service shape
    // `Kinesis_20131202`.
    Api {
        namespace: "kinesis",
        signing_name: "kinesis",
        endpoint_prefix: "kinesis",
        target_prefix: "Kinesis_20131202",
        json_version: "1.1",
        sdk_id: "Kinesis",
        // `PutRecord`'s input: `StreamName`, `Data`, `PartitionKey`.
        member_case: MemberCase::Pascal,
    },
    // `services/service-com-amazonaws-ssm-amazonssm/smithy-model/
    // com-amazonaws-ssm-amazonssm.json`: awsJson1_1, sigv4Name `ssm`,
    // endpointPrefix `ssm`, sdkId `SSM`, service shape `AmazonSSM`.
    Api {
        namespace: "ssm",
        signing_name: "ssm",
        endpoint_prefix: "ssm",
        target_prefix: "AmazonSSM",
        json_version: "1.1",
        sdk_id: "SSM",
        // `GetParameter`'s input: `Name`, `WithDecryption`.
        member_case: MemberCase::Pascal,
    },
    // `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/
    // smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json`:
    // awsJson1_1, sigv4Name `ecs`, endpointPrefix `ecs`, sdkId `ECS`, service shape
    // `AmazonEC2ContainerServiceV20141113`.
    Api {
        namespace: "ecs",
        signing_name: "ecs",
        endpoint_prefix: "ecs",
        target_prefix: "AmazonEC2ContainerServiceV20141113",
        json_version: "1.1",
        sdk_id: "ECS",
        // `RunTask`'s input: `cluster`, `taskDefinition`, `overrides`.
        member_case: MemberCase::LowerCamel,
    },
    // `services/service-com-amazonaws-glue-awsglue/smithy-model/
    // com-amazonaws-glue-awsglue.json`: awsJson1_1, sigv4Name `glue`, endpointPrefix
    // `glue`, sdkId `Glue`, service shape `AWSGlue`.
    Api {
        namespace: "glue",
        signing_name: "glue",
        endpoint_prefix: "glue",
        target_prefix: "AWSGlue",
        json_version: "1.1",
        sdk_id: "Glue",
        // `StartJobRun`'s input: `JobName`, `Arguments`, `Timeout`.
        member_case: MemberCase::Pascal,
    },
];

/// One integration call: the service's API, and the operation of it the `Resource`'s
/// action named.
#[derive(Debug, Clone)]
pub struct Call {
    pub api: &'static Api,
    /// The operation name as the service's model spells it, derived from the action.
    pub operation: String,
    /// The run-a-job pattern this call STARTS, for a `Resource` that asked for one
    /// (`.sync`, `.sync:2`): the task is finished when the job this call starts is,
    /// so the walk parks at it instead of placing the response as the result.
    /// `None` for a request-response integration, whose call IS the whole task.
    pub job: Option<&'static RunAJob>,
    /// Whether the `Resource` asked for the pattern's SECOND revision (`.sync:2`),
    /// which differs in one thing: a document the job carries as JSON TEXT is
    /// reported as the parsed document rather than as that text
    /// ([`RunAJob::json_text_at`]).
    pub parsed_output: bool,
    /// The members of this call's request the invoked operation types as a STRING
    /// while the integration documents them as a document, so the document the
    /// state built is carried as that document's JSON text
    /// ([`JSON_TEXT_MEMBERS`]). Empty for every call that converts nothing.
    pub json_text: &'static [&'static str],
    /// Whether the task's result is the literal SDK response of this call, which
    /// carries the SDK's own response envelope beside the operation's members
    /// ([`with_sdk_envelope`]).
    ///
    /// True for the OPTIMIZED integration family, whose result is that SDK response
    /// as it stands. False for the AWS SDK family, whose result is the invoked
    /// operation's own members, and for the calls a run-a-job pattern makes to REPORT
    /// and to STOP its job: those are not a task's own work, so what they answer is
    /// read rather than reported.
    pub sdk_envelope: bool,
    /// The members of this call's RESPONSE that carry an instant, which the result
    /// reports in whole epoch milliseconds ([`EPOCH_MILLIS_MEMBERS`]). Empty for every
    /// call that converts nothing.
    pub epoch_millis: &'static [&'static str],
    /// Where this call's request names the resource it acts ON, for the integrations
    /// whose target is named by the request ([`TARGET_RESOURCES`]); `None` for a call
    /// no such entry is carried for.
    pub targets: Option<&'static TargetResource>,
}

/// Where an integration's request names the resource its call acts ON, and how a
/// refusal of that call is reported to the `Task` state.
///
/// A `Task` state's integration call is made AS the state machine's execution role,
/// so it is subject to that role's own policies — an IAM access request over an
/// `(action, resource)` pair. The ACTION is the IAM action the operation publishes,
/// which is the invoked service's action namespace (an [`Api`]'s `arnNamespace`) and
/// the operation's own name, so nothing is stated per action here. The RESOURCE is
/// the integration's own: which member of the request names it is a fact of that
/// integration, which is why there is one entry per integration rather than a rule.
#[derive(Debug)]
pub struct TargetResource {
    /// The `<namespace>:<action>` the OPTIMIZED family names this integration by.
    pub integration: &'static str,
    /// The member of the integration's request that names the resource the call acts
    /// on, named in the integration's own PascalCase.
    pub member: &'static str,
    /// How that member spells the resource it names ([`TargetNaming`]).
    pub named: TargetNaming,
    /// The error name a `Task` state reports when the invoked service refuses this
    /// call with an error its own operation does not model: the service as Step
    /// Functions names it in a task error, and that service's base SDK exception.
    pub refusal_error: &'static str,
    /// The name that refusal's own metadata names the service under, which is the
    /// name its SDK client carries rather than anything in its model.
    pub refusal_service: &'static str,
}

/// How the member of an integration's request that names the call's target SPELLS it.
///
/// An IAM access request is over the resource's ARN, whatever the operation's own
/// member takes: an operation that addresses its resource by ARN carries the resource
/// itself, while one that addresses it by NAME carries a name that is the ARN's last
/// field, and the ARN it names is this account's own ARN for that resource in the
/// Region the call is made in. Which of the two a member is, is a fact of the invoked
/// operation's model, so it is stated per entry rather than guessed from the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetNaming {
    /// The member carries the resource's ARN, so it IS the resource the access
    /// request is over.
    Arn,
    /// The member carries the resource's NAME inside one account and Region, so the
    /// ARN it names is minted from it. A value already written as an ARN is the
    /// resource itself, because such a member is one the invoked operation takes
    /// either way.
    Named {
        /// The ARN namespace of the service the resource belongs to.
        service: &'static str,
        /// What precedes the name in the ARN's resource field, the resource type the
        /// invoked service writes it under (`table/`).
        prefix: &'static str,
    },
}

/// The integrations whose request names the resource their call acts on.
///
/// One entry per integration, because which member names the target — and whether it
/// names it by ARN or by name — is that integration's own fact. An entry also needs
/// the invoked service's base SDK exception, the one a refusal its operation does not
/// model is reported under, so an entry is added from that service's own model and
/// SDK rather than guessed at. An integration with no entry is one no access request
/// is formed for, and the call is made as before.
///
/// An entry is keyed by the `<namespace>:<action>` the OPTIMIZED family writes, and
/// serves BOTH families: the AWS SDK family names the same service and the same
/// action, so it makes the same call on the same resource and is subject to the same
/// access request. Only the SPELLING of the request's members differs between them,
/// which [`TargetResource::resource_of`] reads either way.
///
/// Nothing here is per ACTION beyond the entry: the IAM action an entry's call is
/// subject to is the invoked service's namespace and the operation's own name
/// ([`access_request`]), so two actions of one service differ only in which member
/// names their target.
pub const TARGET_RESOURCES: &[TargetResource] = &[
    // `arn:aws:states:::states:startExecution` starts an execution of the state
    // machine its `StateMachineArn` parameter names, which is the resource
    // `states:StartExecution` is taken on
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model: `stateMachineArn` is the ARN "of the state machine to execute").
    // A refusal this operation does not model is reported under this service's own
    // base SDK exception, the one the AWS SDKs deserialize an unmodelled Step
    // Functions error into.
    TargetResource {
        integration: "states:startExecution",
        member: "StateMachineArn",
        named: TargetNaming::Arn,
        refusal_error: "StepFunctions.AWSStepFunctionsException",
        refusal_service: "AWSStepFunctions",
    },
    // DynamoDB's item-level integrations each act on ONE table, the one their
    // `TableName` parameter names: "The name of the table to contain the item" /
    // "containing the requested item" / "from which to delete the item"
    // (`smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,
    // DeleteItem",model_path="services/
    // service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json")`). The member is the table's
    // NAME rather than its ARN, and a table's ARN writes that name under the `table`
    // resource type — "The Amazon Resource Name (ARN) that uniquely identifies the
    // table" (`TableDescription$TableArn` of the same model, which that service mints
    // as `table/<name>`) — so the resource the access request is over is minted from
    // it. A refusal none of these operations models is reported under DynamoDB's base
    // SDK exception.
    TargetResource {
        integration: "dynamodb:putItem",
        member: "TableName",
        named: DYNAMODB_TABLE,
        refusal_error: DYNAMODB_REFUSAL_ERROR,
        refusal_service: DYNAMODB_REFUSAL_SERVICE,
    },
    TargetResource {
        integration: "dynamodb:getItem",
        member: "TableName",
        named: DYNAMODB_TABLE,
        refusal_error: DYNAMODB_REFUSAL_ERROR,
        refusal_service: DYNAMODB_REFUSAL_SERVICE,
    },
    TargetResource {
        integration: "dynamodb:updateItem",
        member: "TableName",
        named: DYNAMODB_TABLE,
        refusal_error: DYNAMODB_REFUSAL_ERROR,
        refusal_service: DYNAMODB_REFUSAL_SERVICE,
    },
    TargetResource {
        integration: "dynamodb:deleteItem",
        member: "TableName",
        named: DYNAMODB_TABLE,
        refusal_error: DYNAMODB_REFUSAL_ERROR,
        refusal_service: DYNAMODB_REFUSAL_SERVICE,
    },
];

/// A DynamoDB table, named by the table name one of its item-level operations takes.
const DYNAMODB_TABLE: TargetNaming = TargetNaming::Named {
    service: "dynamodb",
    prefix: "table/",
};

/// The error name a `Task` state reports for a DynamoDB refusal the invoked operation
/// does not model, and the name that refusal's metadata names the service under: the
/// service as Step Functions writes it in a task error, and DynamoDB's base SDK
/// exception and client name.
const DYNAMODB_REFUSAL_ERROR: &str = "DynamoDB.AmazonDynamoDBException";
const DYNAMODB_REFUSAL_SERVICE: &str = "AmazonDynamoDBv2";

impl TargetResource {
    /// The ARN of the resource this call's `request` names, in the Region `region`
    /// the call is made in.
    ///
    /// The member is read under both spellings a definition may write it in — the
    /// integration's own PascalCase and the invoked model's ([`MemberCase`]) — because
    /// either reaches the operation ([`wire_request`]). What the value spells is the
    /// entry's own fact ([`TargetNaming`]): a NAME is the last field of this account's
    /// ARN for that resource in this Region, and an ARN is the resource itself.
    fn resource_of(&self, request: &Value, region: &str) -> Option<String> {
        let named = [MemberCase::Pascal, MemberCase::LowerCamel]
            .into_iter()
            .find_map(|case| request.get(case.spell(self.member)))
            .and_then(Value::as_str)?;
        if named.is_empty() {
            return None;
        }
        match self.named {
            TargetNaming::Arn => Some(named.to_string()),
            // A member that takes a name takes an ARN too, so one already written as
            // an ARN names the resource it names rather than a resource of that name.
            TargetNaming::Named { service, prefix } => {
                Some(match ::service_common::arn::parse(named) {
                    Some(_) => named.to_string(),
                    None => ::engine::emulator_arn(service, region, &format!("{prefix}{named}")),
                })
            }
        }
    }
}

/// The access request this call is subject to: the IAM action the invoked operation
/// publishes, and the ARN of the resource the request names.
///
/// `region` is the Region the call is made in, which is where a resource the request
/// named by NAME lives.
///
/// `None` when no [`TARGET_RESOURCES`] entry is carried for the integration, or when
/// the request names no resource at all — a request the invoked operation will itself
/// refuse, which is its own answer to give rather than an authorization decision.
pub fn access_request(call: &Call, request: &Value, region: &str) -> Option<(String, String)> {
    let targets = call.targets?;
    let resource = targets.resource_of(request, region)?;
    Some((
        format!("{}:{}", call.api.namespace, call.operation),
        resource,
    ))
}

/// The members an OPTIMIZED integration documents as a document while the operation
/// it calls types them as a string, named by the WIRE spelling of the member
/// ([`MemberCase`]) under the `<namespace>:<action>` the optimized family names the
/// integration by.
///
/// Only that family converts anything: an AWS SDK integration's parameters ARE the
/// invoked operation's own members, so a document written for one goes on the wire
/// as it stands.
const JSON_TEXT_MEMBERS: &[(&str, &[&str])] = &[
    // `arn:aws:states:::states:startExecution` takes the child execution's input as
    // the JSON document the child receives, while `StartExecution`'s own `input`
    // member is "the string that contains the JSON input data for the execution"
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model).
    ("states:startExecution", &["input"]),
];

/// The members of an integration's RESPONSE that carry an INSTANT, named by the
/// spelling the invoked model gives them.
///
/// A `Task` state reads the SDK response of its call, and the SDK renders a timestamp
/// as its own date type — whole MILLISECONDS since the epoch — while the `awsJson`
/// protocol carries one on the wire as epoch SECONDS with a fractional part. The
/// members are the invoked model's own, which this crate cannot read for another
/// service, so there is one entry per integration and a later entry is added beside
/// them rather than guessed at; an integration with no entry converts nothing.
const EPOCH_MILLIS_MEMBERS: &[(&str, &[&str])] = &[
    // `StartExecution`'s output is `executionArn` and `startDate`, "the date the
    // execution is started", a Timestamp
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model).
    ("states:startExecution", &["startDate"]),
];

/// The suffix a `Resource` names the run-a-job integration pattern with, and the
/// revision of it that parses the job's own output
/// (`arn:aws:states:::states:startExecution.sync:2`).
const RUN_A_JOB_PATTERN: &str = "sync";
const RUN_A_JOB_PATTERN_2: &str = "sync:2";

/// The `.waitForTaskToken` pattern, whose outcome is a `SendTaskSuccess` from
/// outside the execution rather than a job this emulator can poll.
const CALLBACK_PATTERN: &str = "waitForTaskToken";

/// One run-a-job (`.sync`) service integration: the three calls the pattern is made
/// of, and where the job's identifier, status and failure sit in their documents.
///
/// The pattern itself is not in any Smithy model — it is Step Functions' own
/// behaviour over another service's API — so each entry names the operations and
/// members of the INVOKED service's model, cited on the entry. Only integrations
/// whose reporting and stopping calls this emulator can really make have an entry:
/// an integration with none is reported by [`read`] as one no job was started for,
/// rather than parked at a job nothing could poll.
#[derive(Debug)]
pub struct RunAJob {
    /// The `<service>:<action>` of the `Resource` that starts the job, as the
    /// optimized integration spells it.
    pub starts: &'static str,
    /// The member of the STARTING call's response that identifies the job.
    pub job_id: &'static str,
    /// The operation that reports the job, and the member of its request the
    /// identifier goes in.
    pub describe: &'static str,
    pub describe_member: &'static str,
    /// Where the job's status sits in the reporting call's response.
    pub status_at: &'static [&'static str],
    /// The status values that mean the job finished its work, and those that mean it
    /// ended without finishing it. A status in neither list is the job still running.
    pub succeeded: &'static [&'static str],
    pub failed: &'static [&'static str],
    /// Where the reporting call's response says WHY a job that ended without
    /// finishing did so; empty where that service reports no such member.
    pub failure_cause_at: &'static [&'static str],
    /// Where the reporting call's response carries a document of the job's own as a
    /// JSON-encoded STRING — its input, its output — each of which the pattern's
    /// second revision reports as the parsed document; empty where that service
    /// carries none.
    pub json_text_at: &'static [&'static [&'static str]],
    /// The operation that stops the job, and the member of its request the
    /// identifier goes in.
    pub stop: &'static str,
    pub stop_member: &'static str,
    /// Whether the stopping call's identifier member is a LIST of identifiers, as
    /// that operation's own model types it — a batch stop takes the ids of several
    /// jobs, and the one job a task started is a batch of one.
    pub stops_in_batch: bool,
    /// The members of the STARTING call's REQUEST that the reporting and stopping
    /// calls carry BESIDE the job's identifier, because those operations do not name
    /// the job by that identifier alone: each is a member of the job's own identity
    /// rather than of the run the identifier names. Named in the integration's own
    /// PascalCase, and empty for a job its identifier names on its own.
    pub names_job_with: &'static [&'static str],
}

/// The members of the starting request that name the job, beside its own identifier
/// ([`RunAJob::names_job_with`]), as the calls that report and stop it carry them.
pub type JobNaming = ::std::collections::BTreeMap<String, String>;

impl RunAJob {
    /// The members of the starting call's `request` that the calls which REPORT and
    /// STOP the job carry beside its identifier.
    ///
    /// Each is read under both spellings a definition may write it in — the
    /// integration's own PascalCase and the invoked model's ([`MemberCase`]) — since
    /// either reaches the operation ([`wire_request`]), and is carried on under the
    /// name this entry gives it. A member the request did not carry is not invented:
    /// the invoked operation answers for a request that omits one, which is its own
    /// answer to give.
    pub fn naming(&self, request: &Value) -> JobNaming {
        let mut naming = JobNaming::new();
        for member in self.names_job_with {
            let named = [MemberCase::Pascal, MemberCase::LowerCamel]
                .into_iter()
                .find_map(|case| request.get(case.spell(member)))
                .and_then(Value::as_str);
            if let Some(value) = named {
                naming.insert((*member).to_string(), value.to_string());
            }
        }
        naming
    }

    /// Whether `status` is one the job ENDED on, and whether it ended having done its
    /// work: `None` while the job is still running.
    fn finished(&self, status: &str) -> Option<bool> {
        if self.succeeded.contains(&status) {
            return Some(true);
        }
        if self.failed.contains(&status) {
            return Some(false);
        }
        None
    }

    /// The value `path` names in `document`, as text.
    fn text_at(document: &Value, path: &[&str]) -> Option<String> {
        let mut node = document;
        for member in path {
            node = node.get(*member)?;
        }
        match node {
            Value::String(text) => Some(text.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        }
    }

    /// The job's status, as the reporting call's `answer` carries it.
    pub fn status_of(&self, answer: &Value) -> Option<String> {
        Self::text_at(answer, self.status_at)
    }

    /// Whether the job `answer` reports has ENDED, and with what: `Ok(answer)` for
    /// one that did its work, `Err(the reason it did not)` for one that did not, and
    /// `None` while it is still running.
    pub fn outcome_of(&self, answer: &Value, parsed_output: bool) -> Option<Result<Value, String>> {
        let status = self.status_of(answer)?;
        match self.finished(&status)? {
            true => Some(Ok(self.reported(answer, parsed_output))),
            false => Some(Err(self.why_it_failed(answer, &status))),
        }
    }

    /// The document the task reports as its result: the reporting call's own
    /// response, with each document the job carries as JSON TEXT parsed in place for
    /// the pattern's second revision.
    fn reported(&self, answer: &Value, parsed_output: bool) -> Value {
        let mut reported = answer.clone();
        if !parsed_output {
            return reported;
        }
        for path in self.json_text_at {
            parse_in_place(&mut reported, path);
        }
        reported
    }

    /// Why a job that ended without doing its work did so: the reason the reporting
    /// call names, else the status it ended in.
    fn why_it_failed(&self, answer: &Value, status: &str) -> String {
        match Self::text_at(answer, self.failure_cause_at) {
            Some(reason) if !reason.is_empty() => reason,
            _ => format!(
                "the '{}' job this task started ended in status {status}",
                self.starts
            ),
        }
    }
}

/// Replace the JSON TEXT `path` names in `document` with the document that text
/// holds, leaving `document` as it is where the path names nothing or names text
/// that is not JSON.
fn parse_in_place(document: &mut Value, path: &[&str]) {
    let Some((last, leading)) = path.split_last() else {
        return;
    };
    let mut node = document;
    for member in leading {
        match node.get_mut(*member) {
            Some(next) => node = next,
            None => return,
        }
    }
    let parsed = node
        .get(*last)
        .and_then(Value::as_str)
        .and_then(|text| ::serde_json::from_str::<Value>(text).ok());
    if let (Some(parsed), Some(object)) = (parsed, node.as_object_mut()) {
        object.insert((*last).to_string(), parsed);
    }
}

/// The run-a-job integrations this emulator performs.
///
/// One entry per `<service>:<action>` that STARTS a job, naming the calls that
/// report and stop it. A `.sync` `Resource` outside this list starts nothing: see
/// [`read`].
pub const RUN_A_JOB: &[RunAJob] = &[
    // A nested execution of a state machine, through this service's own API:
    // `StartExecution` answers `executionArn`, `DescribeExecution` reports `status`
    // (`ExecutionStatus`: RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED) beside
    // `error`, `cause` and the string `output`, and `StopExecution` stops it
    // (`smithy_query(query="operation",operation="StartExecution,DescribeExecution,
    // StopExecution")` on this crate's own model).
    RunAJob {
        starts: "states:startExecution",
        job_id: "executionArn",
        describe: "DescribeExecution",
        describe_member: "executionArn",
        status_at: &["status"],
        succeeded: &[super::EXECUTION_STATUS_SUCCEEDED],
        failed: &[
            super::EXECUTION_STATUS_FAILED,
            super::EXECUTION_STATUS_TIMED_OUT,
            super::EXECUTION_STATUS_ABORTED,
        ],
        failure_cause_at: &["cause"],
        // `DescribeExecution` carries both the execution's `input` and its `output`
        // as "the string that contains the JSON input data" and the JSON output of
        // the execution.
        json_text_at: &[&["input"], &["output"]],
        stop: "StopExecution",
        stop_member: "executionArn",
        stops_in_batch: false,
        // `DescribeExecution` and `StopExecution` each name the execution by its own
        // ARN alone.
        names_job_with: &[],
    },
    // An Athena query: `StartQueryExecution` answers `QueryExecutionId`,
    // `GetQueryExecution` reports `QueryExecution.Status.State`
    // (`QueryExecutionState`: QUEUED, RUNNING, SUCCEEDED, FAILED, CANCELLED) beside
    // `StateChangeReason`, and `StopQueryExecution` stops it
    // (`smithy_query(query="operation",operation="StartQueryExecution,
    // GetQueryExecution,StopQueryExecution",model_path="services/
    // service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json")`).
    RunAJob {
        starts: "athena:startQueryExecution",
        job_id: "QueryExecutionId",
        describe: "GetQueryExecution",
        describe_member: "QueryExecutionId",
        status_at: &["QueryExecution", "Status", "State"],
        succeeded: &["SUCCEEDED"],
        failed: &["FAILED", "CANCELLED"],
        failure_cause_at: &["QueryExecution", "Status", "StateChangeReason"],
        json_text_at: &[],
        stop: "StopQueryExecution",
        stop_member: "QueryExecutionId",
        stops_in_batch: false,
        // `GetQueryExecution` and `StopQueryExecution` each name the query by its own
        // id alone.
        names_job_with: &[],
    },
    // A Glue job run: `StartJobRun` answers `JobRunId`, `GetJobRun` reports
    // `JobRun.JobRunState` (`JobRunState`: STARTING, RUNNING, STOPPING, STOPPED,
    // SUCCEEDED, FAILED, TIMEOUT, ERROR, WAITING, EXPIRED) beside
    // `JobRun.ErrorMessage`, and `BatchStopJobRun` stops it. Both of those name the
    // run by the `JobName` of the job it is a run of — `@required` on each of them —
    // beside the run's own id, which `GetJobRun` takes as `RunId` and
    // `BatchStopJobRun` as the `JobRunIds` LIST
    // (`smithy_query(query="operation",operation="StartJobRun,GetJobRun,
    // BatchStopJobRun",model_path="services/service-com-amazonaws-glue-awsglue/
    // smithy-model/com-amazonaws-glue-awsglue.json")`).
    RunAJob {
        starts: "glue:startJobRun",
        job_id: "JobRunId",
        describe: "GetJobRun",
        describe_member: "RunId",
        status_at: &["JobRun", "JobRunState"],
        succeeded: &["SUCCEEDED"],
        // The `JobRunState` values a run never moves out of again; `STARTING`,
        // `RUNNING`, `STOPPING` and `WAITING` each still owe a further state.
        failed: &["STOPPED", "FAILED", "TIMEOUT", "ERROR", "EXPIRED"],
        failure_cause_at: &["JobRun", "ErrorMessage"],
        json_text_at: &[],
        stop: "BatchStopJobRun",
        stop_member: "JobRunIds",
        stops_in_batch: true,
        names_job_with: &["JobName"],
    },
];

/// What a task's own work failed with: the error NAME the state reports (which its
/// `Retry` and `Catch` are matched against) and the cause that says what happened.
#[derive(Debug, Clone)]
pub struct Failed {
    pub error: String,
    pub cause: String,
}

/// What one integration call answered: the response document, which becomes the
/// state's result, or the failure the state reports.
pub type Answer = Result<Value, Failed>;

/// What performs a `Task` state's service integration for one walk.
///
/// The walk is synchronous — it runs on a blocking thread and its states are a
/// function of the execution's own data — while an API call is asynchronous, so the
/// walk asks for a call through this seam and the asynchronous operation running it
/// ([`interpret`]) makes it.
pub trait Tasks: Send + Sync {
    /// Make `call` with `request` as the operation's input document, waiting at most
    /// `within` for it to answer.
    ///
    /// `within` is the state's own `TimeoutSeconds` when it declares one — "if the
    /// task runs longer than the specified seconds, this state fails with a
    /// States.Timeout error name" — so the member bounds the span it documents
    /// rather than only being compared with it afterwards. `None` leaves the
    /// emulator's own ceiling ([`MAX_CALL_DURATION`]) as the only bound, and a
    /// declared timeout longer than that ceiling does not lift it.
    fn perform(&self, call: &Call, request: &Value, within: Option<Duration>) -> Answer;

    /// Why the execution role this walk runs as may NOT make `call` with `request`,
    /// or `None` when nothing of the kind can be shown.
    ///
    /// A `Task` state's integration call is made as the state machine's execution
    /// role, so a call that role's policies do not allow never happens: the walk asks
    /// this BEFORE [`Tasks::perform`] and fails the task with what this answers. The
    /// default shows nothing, for a walk with no role to judge the call against.
    fn authorize(&self, _call: &Call, _request: &Value) -> Option<Failed> {
        None
    }
}

/// A [`Tasks`] that makes no call, for a walk with no operation around it to make
/// one (`TestState`, which stands a mock in for the invocation instead).
#[derive(Debug, Default)]
pub struct Unserved;

impl Tasks for Unserved {
    fn perform(&self, call: &Call, _request: &Value, _within: Option<Duration>) -> Answer {
        Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "the '{}' call of {} is not made outside an execution, so nothing was run for \
                 this task",
                call.operation, call.api.sdk_id
            ),
        })
    }
}

/// The integration call `resource` names, or why no call is made for it.
///
/// The `Err` is the cause an execution that reaches such a state fails with: it says
/// what the `Resource` named and what about it was not run, and never claims the
/// work happened.
pub fn read(resource: &str) -> Result<Call, String> {
    let shown = super::asl::echoed(resource);
    let fields: Vec<&str> = resource.splitn(6, ':').collect();
    let (vendor, rest) = match fields.as_slice() {
        ["arn", _partition, vendor, _region, _account, rest] => (*vendor, *rest),
        _ => {
            return Err(format!(
                "'{shown}' is not an ARN, so it names no service integration and nothing was run \
                 for it"
            ))
        }
    };
    if vendor != STATES_NAMESPACE {
        return Err(format!(
            "'{shown}' names a resource of '{}' rather than a service integration, which this \
             emulator does not invoke directly, so nothing was run for it",
            super::asl::echoed(vendor)
        ));
    }
    // `aws-sdk:<service>:<action>` for the AWS SDK family, `<service>:<action>` for
    // the optimized one: both name a service and an action, which is all the call
    // needs.
    let (aws_sdk, (service, action)) = match rest.split_once(':') {
        Some((AWS_SDK_SEGMENT, tail)) => (
            true,
            tail.split_once(':').ok_or_else(|| {
                format!("'{shown}' names no action of its service, so nothing was run for it")
            })?,
        ),
        Some(pair) => (false, pair),
        None => {
            return Err(format!(
                "'{shown}' names no action of its service, so nothing was run for it"
            ))
        }
    };
    if service == super::ACTIVITY_RESOURCE {
        return Err(format!(
            "'{shown}' names an activity, whose work is handed to a worker that polls \
             'GetActivityTask' for it rather than being an integration call, so no call was \
             made and nothing was run for it"
        ));
    }
    // "<action>.sync", "<action>.sync:2" and "<action>.waitForTaskToken" name an
    // integration PATTERN after the action: the action is what is called either way,
    // and the pattern says what finishes the task.
    let (action, pattern) = match action.split_once('.') {
        Some((action, pattern)) => (action, Some(pattern)),
        None => (action, None),
    };
    if action.is_empty() {
        return Err(format!(
            "'{shown}' names no action of its service, so nothing was run for it"
        ));
    }
    // Each family names the service by its OWN token: the optimized one by the
    // service's ARN namespace, the AWS SDK one by the name the SDKs give it
    // ([`Api::sdk_token`]).
    let api = APIS
        .iter()
        .find(|api| match aws_sdk {
            true => api.sdk_token() == service,
            false => api.namespace == service,
        })
        .ok_or_else(|| {
            format!(
                "'{shown}' names an integration with '{}', which is not one of the services this \
                 emulator makes a 'Task' state's integration call to, so nothing was run for it",
                super::asl::echoed(service)
            )
        })?;
    // The run-a-job pattern is performed for the integrations [`RUN_A_JOB`] names
    // the reporting and stopping calls of; the callback pattern waits for a
    // `SendTaskSuccess` nothing inside the execution makes.
    let (job, parsed_output) = match pattern {
        None => (None, false),
        Some(pattern @ (RUN_A_JOB_PATTERN | RUN_A_JOB_PATTERN_2)) => {
            let starts = format!("{service}:{action}");
            let job = RUN_A_JOB
                .iter()
                .find(|job| job.starts == starts)
                .ok_or_else(|| {
                    format!(
                        "'{shown}' asks for the run-a-job ('{}') pattern of an integration whose \
                         job this emulator cannot report or stop, so no job was started and \
                         nothing was run for it",
                        super::asl::echoed(pattern)
                    )
                })?;
            (Some(job), pattern == RUN_A_JOB_PATTERN_2)
        }
        Some(pattern) if pattern == CALLBACK_PATTERN => {
            return Err(format!(
                "'{shown}' asks for the '{}' integration pattern, whose outcome is a later \
                 'SendTaskSuccess' from outside the execution, so nothing was run for it",
                super::asl::echoed(pattern)
            ))
        }
        Some(pattern) => {
            return Err(format!(
                "'{shown}' names '{}', which is not one of this service's integration patterns, \
                 so nothing was run for it",
                super::asl::echoed(pattern)
            ))
        }
    };
    // The conversion a member of this call's request needs is the integration's,
    // so it is read off the optimized `<namespace>:<action>` and is empty for the
    // AWS SDK family, whose parameters are the operation's own members.
    let integration = format!("{}:{action}", api.namespace);
    let json_text = match aws_sdk {
        true => &[][..],
        false => members_of(JSON_TEXT_MEMBERS, &integration),
    };
    // The same for the instants its RESPONSE carries: the optimized family's result
    // is the SDK response, which renders a date as epoch milliseconds.
    let epoch_millis = match aws_sdk {
        true => &[][..],
        false => members_of(EPOCH_MILLIS_MEMBERS, &integration),
    };
    Ok(Call {
        api,
        operation: operation_of(action),
        job,
        parsed_output,
        json_text,
        // The OPTIMIZED family's result is the literal SDK response; the AWS SDK
        // family's is the invoked operation's own members.
        sdk_envelope: !aws_sdk,
        epoch_millis,
        // What the call acts ON is the same resource whichever family named it, so
        // the entry is read for both.
        targets: TARGET_RESOURCES
            .iter()
            .find(|entry| entry.integration == integration),
    })
}

/// The members `table` names for the integration `integration`, and none when it
/// names it at all.
fn members_of(
    table: &'static [(&'static str, &'static [&'static str])],
    integration: &str,
) -> &'static [&'static str] {
    table
        .iter()
        .find(|(named, _)| *named == integration)
        .map_or(&[][..], |(_, members)| *members)
}

/// The call that reports the job `job` started under `job_id`, and its request.
///
/// `naming` is what the starting call's own request said the job belongs to
/// ([`RunAJob::naming`]), which this call carries beside the identifier for an
/// integration whose reporting operation needs it.
pub fn describing(
    api: &'static Api,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    (
        pattern_call(api, job.describe),
        job_request(
            job.describe_member,
            Value::String(job_id.to_string()),
            naming,
        ),
    )
}

/// The call that STOPS the job `job` started under `job_id`, and its request.
///
/// `naming` is as [`describing`]'s. The identifier goes in as the stopping operation
/// types that member: a batch stop takes a LIST of ids, and the one job a task started
/// is a batch of one ([`RunAJob::stops_in_batch`]).
pub fn stopping(
    api: &'static Api,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    let identifier = match job.stops_in_batch {
        true => Value::Array(vec![Value::String(job_id.to_string())]),
        false => Value::String(job_id.to_string()),
    };
    (
        pattern_call(api, job.stop),
        job_request(job.stop_member, identifier, naming),
    )
}

/// The request one of a run-a-job pattern's own calls is made with: the job's
/// identifier under `member`, beside every member of the starting request that names
/// the job it belongs to.
fn job_request(member: &str, identifier: Value, naming: &JobNaming) -> Value {
    let mut request = Map::new();
    request.insert(member.to_string(), identifier);
    for (member, value) in naming {
        request.insert(member.clone(), Value::String(value.clone()));
    }
    Value::Object(request)
}

/// The call one of a run-a-job pattern's own calls is made as.
///
/// The job's REPORT and its STOP are READ, not reported as a task's own result, so
/// neither parks at a job of its own, converts a member of its answer or carries the
/// integration's response envelope.
fn pattern_call(api: &'static Api, operation: &str) -> Call {
    Call {
        api,
        operation: operation.to_string(),
        job: None,
        parsed_output: false,
        json_text: &[],
        sdk_envelope: false,
        epoch_millis: &[],
        targets: None,
    }
}

/// The operation name an action names: an action is the operation's name in
/// lowerCamel (`putItem` -> `PutItem`), so the operation is the action with its
/// first character upper-cased and nothing else changed.
fn operation_of(action: &str) -> String {
    let mut characters = action.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// How a `Task` state's own history events name the work its `Resource` asked for:
/// "the service name of the resource in a task state" and "the action of the resource
/// called by a task state"
/// (`smithy_query(query="shape",shape="TaskScheduledEventDetails")`).
///
/// A `Resource` names the integration as `<kind>:<action>`, where the kind is the
/// service alone for the optimized family (`lambda:invoke`) and the family plus the
/// service for the AWS SDK one (`aws-sdk:dynamodb:putItem`): the KIND is read from
/// the FRONT, so the action is everything after it — which is what a `Resource`
/// naming an integration PATTERN carries, since a pattern's own revision is written
/// with a colon (`startExecution.sync:2`) and belongs to the action rather than
/// being a field of its own. A `Resource` with no action at all is its own kind,
/// which is what a state naming work no call is made for reports.
pub fn named_work(resource: &str) -> (String, String) {
    let rest = match resource.splitn(6, ':').collect::<Vec<&str>>().as_slice() {
        ["arn", _partition, _vendor, _region, _account, rest] => (*rest).to_string(),
        _ => resource.to_string(),
    };
    // The AWS SDK family's kind is two fields (`aws-sdk:<service>`), every other
    // kind is one, and the action is the rest of the text either way.
    let split = match rest.split_once(':') {
        Some((AWS_SDK_SEGMENT, tail)) => tail
            .split_once(':')
            .map(|(service, action)| (format!("{AWS_SDK_SEGMENT}:{service}"), action.to_string())),
        Some((kind, action)) => Some((kind.to_string(), action.to_string())),
        None => None,
    };
    let (kind, action) = match split {
        Some((kind, action)) if !kind.is_empty() && !action.is_empty() => (kind, action),
        _ => (rest.clone(), rest),
    };
    (clip_name(kind), clip_name(action))
}

/// `name` within the bytes the modelled `Name` shape (len 1..80) carries, cut at a
/// character boundary: a `Resource` is a caller's text, so what it names is reported
/// through the member's own bound rather than past it.
fn clip_name(name: String) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name;
    }
    let mut end = MAX_NAME_BYTES;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// One integration call handed from the walk to the operation that is running it.
struct Handover {
    call: Call,
    request: Value,
    /// Where the answer goes. A std channel, not a `oneshot`, because the side
    /// WAITING for it is a blocking thread that must not wait forever: this is the
    /// one receiver with a timeout ([`MAX_CALL_DURATION`]). Sending never blocks.
    answer: ::std::sync::mpsc::Sender<Answer>,
}

/// The [`Tasks`] a walk inside an operation performs its calls through: each call is
/// handed to that operation's own asynchronous task, which makes it and hands the
/// answer back.
///
/// One of these exists only INSIDE the blocking closure [`interpret`] spawns — it is
/// private, its field is private, and nothing else constructs one — which is what
/// makes the blocking handover below sound: a blocking send from an async worker
/// would be a defect, and there is no way to reach this from one.
struct Dispatcher {
    calls: ::tokio::sync::mpsc::Sender<Handover>,
    /// How deep a chain of in-process calls this interpretation is already running
    /// in — the depth the request that started it arrived at — against
    /// [`MAX_CALL_CHAIN_DEPTH`].
    depth: u8,
    /// What decides whether the execution role this walk runs as may make a call,
    /// when the call that started the walk could resolve that role; `None` leaves
    /// every call unjudged, which is what a walk with no role to ask about does.
    role: Option<RoleGate>,
}

/// What decides whether the execution role a walk runs as may make one integration
/// call: the `(action, resource)` the call is subject to is formed from the call
/// itself ([`access_request`]), and this answers the refusal the task reports when
/// the policies in force on the role do not allow it.
///
/// A closure rather than a trait, because the documents it reads live in another
/// service's store and are reached through the engine the operation holds
/// ([`super::task_authorization`]): the walk knows nothing of either, and asks only
/// the question.
pub type RoleGate = ::std::sync::Arc<dyn Fn(&Call, &Value) -> Option<Failed> + Send + Sync>;

/// What a task fails with when the call could not be handed over or answered at all
/// — the walk and the operation running it parted company, which is this emulator
/// failing to ask rather than the service refusing.
fn unavailable(operation: &str) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "the '{operation}' call this task makes could not be handed to the request running \
             the execution, so nothing was run for it"
        ),
    }
}

/// What a task fails with when the chain of in-process calls it sits in is already
/// as deep as this emulator carries one ([`MAX_CALL_CHAIN_DEPTH`]): the call is NOT
/// made, so the thread this walk holds is the last one the chain pins.
fn too_deep(call: &Call) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "the '{}' call of {} would be made from a chain of in-process calls already \
             {MAX_CALL_CHAIN_DEPTH} deep, which is as deep as this emulator carries one, so \
             nothing was run for it",
            call.operation, call.api.sdk_id
        ),
    }
}

/// What a task fails with when its call did not answer inside the span it was given
/// (`waited`): ASL's own timeout error, which the state's `Retry` and `Catch` may
/// match, rather than a thread held until the call feels like answering.
///
/// `waited` is the state's own `TimeoutSeconds` when it declares one no longer than
/// [`MAX_CALL_DURATION`], and that ceiling otherwise, so the cause states the bound
/// that actually expired.
fn timed_out(call: &Call, waited: Duration) -> Failed {
    Failed {
        error: super::asl::TIMEOUT_ERROR.to_string(),
        cause: format!(
            "the '{}' call of {} did not answer inside the {:?} this task waits for one",
            call.operation, call.api.sdk_id, waited
        ),
    }
}

impl Tasks for Dispatcher {
    fn perform(&self, call: &Call, request: &Value, within: Option<Duration>) -> Answer {
        // The chain's depth is decided BEFORE the handover: a call this deep is not
        // made at all, so the chain stops pinning threads here.
        if self.depth >= MAX_CALL_CHAIN_DEPTH {
            return Err(too_deep(call));
        }
        // The state's own `TimeoutSeconds` bounds the call when it asks for less
        // than the emulator's ceiling; it cannot ask for more.
        let waited = within.map_or(MAX_CALL_DURATION, |timeout| timeout.min(MAX_CALL_DURATION));
        let (answer, answered) = ::std::sync::mpsc::channel();
        let handover = Handover {
            call: call.clone(),
            request: request.clone(),
            answer,
        };
        // The walk runs on a blocking thread (`spawn_blocking`), never on an async
        // worker, so it may block here. The handover itself cannot wait on a full
        // channel: the walk has at most one call outstanding, and a channel whose
        // serving half is gone refuses the send rather than blocking.
        self.calls
            .blocking_send(handover)
            .map_err(|_| unavailable(&call.operation))?;
        // The wait is CLAMPED server-side: whatever the call is doing, this thread
        // is released at that bound.
        match answered.recv_timeout(waited) {
            Ok(answer) => answer,
            Err(::std::sync::mpsc::RecvTimeoutError::Timeout) => Err(timed_out(call, waited)),
            Err(::std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(unavailable(&call.operation))
            }
        }
    }

    fn authorize(&self, call: &Call, request: &Value) -> Option<Failed> {
        self.role.as_ref()?(call, request)
    }
}

/// Run `work` — one interpretation of a definition — on a blocking thread, serving
/// the integration calls its `Task` states make from this asynchronous call.
///
/// The walk is CPU work a caller's definition sizes, so it does not run on an async
/// worker; the calls it asks for are asynchronous, so they are made here, on the
/// operation's own task, with the walk waiting for each answer. Both halves are
/// polled together, and the serving half ends when the walk drops its sender — so
/// the call returns exactly when the interpretation is over.
///
/// The thread the walk holds is held ACROSS each call, so how deep a chain of such
/// calls may go is what bounds the threads one inbound request can pin: the chain's
/// depth is read off the request that started this interpretation and carried into
/// the walk, where [`MAX_CALL_CHAIN_DEPTH`] decides whether another call is made.
///
/// `region` is the Region the calls are made in: the execution's own, which is the
/// Region its state machine's ARN names.
pub async fn interpret<Work, Outcome>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role: Option<RoleGate>,
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    interpret_through(ctx.callback(), chain_depth(ctx), region, role, work).await
}

/// As [`interpret`], from a callback handle and a chain depth rather than from a
/// request's own context.
///
/// A run-a-job pattern's outcome is not served by the request that started it: the
/// job is polled after that call answered, so the task that polls it holds its own
/// clone of the callback ([`super::run_a_job`]) and interprets the continuation
/// through this. Everything else is identical, including the bound on how deep a
/// chain of in-process calls may still make one.
pub async fn interpret_through<Work, Outcome>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    depth: u8,
    region: &str,
    role: Option<RoleGate>,
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    // One call is in flight at a time: the walk waits for each answer before it
    // transitions, so a deeper channel would buffer nothing.
    let (calls, mut handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    let walk = ::tokio::task::spawn_blocking(move || {
        let dispatcher = Dispatcher {
            calls,
            depth,
            role,
        };
        work(&dispatcher)
    });
    let serve = async {
        while let Some(handover) = handovers.recv().await {
            let answer = call_api(callback, region, &handover.call, &handover.request).await;
            // A walk that has stopped waiting — it timed out, or it is gone — cannot
            // be told; it reported the timeout itself.
            let _ = handover.answer.send(answer);
        }
    };
    let (walked, ()) = ::tokio::join!(walk, serve);
    walked
}

/// How deep a chain of in-process calls the request being served arrived at.
///
/// The framework stamps the depth on every request a handler's callback sends and
/// carries it in the request's extensions, so a request from outside the emulator
/// reads zero and each nested one reads one more
/// (`vendor/smithy-server-runtime/src/operation.rs`). This is the SAME counter the
/// router bounds a cycle with, read here for the resource that counter does not
/// bound: the blocking thread each level of the chain holds.
///
/// Public, because the task that polls a parked job interprets the continuation
/// outside the request that started the job and carries that request's depth into it.
pub fn chain_depth(ctx: &smithy_server_runtime::OperationContext<'_>) -> u8 {
    ctx.parts()
        .extensions
        .get::<smithy_server_runtime::CallbackRecursionDepth>()
        .map_or(0, |depth| depth.0)
}

/// The document a `Task` state REPORTS as the result of its integration call:
/// `answer`'s top-level members under the spelling a service integration's result is
/// written in, which is PascalCase.
///
/// The result of a service integration is the invoked operation's response document,
/// and a `Task` state reads it in the integration's own convention whatever the
/// invoked model spells its own members in — so a `ResultSelector`, a `ResultPath`
/// or a later state's path reads `$.ExecutionArn` of an operation whose own member is
/// `executionArn`. The wire spelling belongs to the call, not to the result.
///
/// Only the TOP level is re-spelled, for the reason [`wire_request`] gives: below it
/// a response carries a caller's own DATA beside the operation's members — the
/// attributes of an item, the output document of a child execution — and data is
/// never re-spelled.
pub fn task_result(answer: &Value) -> Value {
    let Some(members) = answer.as_object() else {
        return answer.clone();
    };
    let mut reported = Map::new();
    for (member, value) in members {
        reported.insert(MemberCase::Pascal.spell(member), value.clone());
    }
    Value::Object(reported)
}

/// The document `request` goes on the wire as: every TOP-LEVEL member under the
/// spelling the invoked service's model uses for it ([`MemberCase`]), and every
/// member the integration documents as a document the operation types as a string
/// carried as that document's JSON text ([`Call::json_text`]).
///
/// A `Task` state's integration parameters are the invoked operation's input, and
/// they are written in the integration's own convention — PascalCase — whatever the
/// invoked API spells its members in, so the spelling is settled here rather than
/// handing a service a member name its model does not carry.
///
/// Only the TOP level is re-spelled. Below it a document carries a caller's DATA
/// beside the operation's members — a DynamoDB attribute name is a map key, an
/// EventBridge detail is a caller's own document — and data is never re-spelled.
/// A value already a string is left as it is, so a member written as the text the
/// operation types goes through untouched.
fn wire_request(call: &Call, request: &Value) -> Value {
    let Some(members) = request.as_object() else {
        return request.clone();
    };
    let mut wire = Map::new();
    for (member, value) in members {
        let member = call.api.member_case.spell(member);
        let value = match call.json_text.contains(&member.as_str()) {
            true if !value.is_string() => Value::String(value.to_string()),
            _ => value.clone(),
        };
        wire.insert(member, value);
    }
    Value::Object(wire)
}

/// Make one integration call and read what the service answered.
///
/// Public, because the calls a run-a-job pattern is made of besides the one the walk
/// asks for — the one that REPORTS the job and the one that STOPS it — are made
/// outside any walk ([`super::run_a_job`]), through this same seam.
pub async fn call_api(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    call: &Call,
    request: &Value,
) -> Answer {
    let api = call.api;
    let body = match ::serde_json::to_vec(&wire_request(call, request)) {
        Ok(body) => body,
        Err(error) => {
            return Err(Failed {
                error: super::asl::RUNTIME_ERROR.to_string(),
                cause: format!(
                    "the document this task would have sent as its request could not be written: \
                     {error}"
                ),
            })
        }
    };
    match dispatch(callback, api, region, &call.operation, body).await {
        Ok(answered) => {
            let read = answer_of(
                api,
                &call.operation,
                answered.status,
                answered.error_type.as_deref(),
                &answered.payload,
            );
            // The result of an OPTIMIZED integration is the literal SDK response of
            // the call, which renders each instant the response carries as whole
            // epoch milliseconds and carries the SDK's own response envelope beside
            // the operation's members; a refusal carries no result to wrap.
            match read {
                Ok(document) if call.sdk_envelope => Ok(with_sdk_envelope(
                    in_epoch_millis(document, call.epoch_millis),
                    &answered,
                )),
                Ok(document) => Ok(in_epoch_millis(document, call.epoch_millis)),
                other => other,
            }
        }
        Err(why) => Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "the '{}' call of {} could not be made: {why}",
                call.operation, api.sdk_id
            ),
        }),
    }
}

/// The member a task's result carries the HTTP facts of the call under, as the SDK
/// response reports them: the status the call answered, and its headers under both
/// spellings the SDK exposes them in.
const SDK_HTTP_METADATA: &str = "SdkHttpMetadata";

/// The member a task's result carries the call's response metadata under: the
/// identifier of the request the call made.
const SDK_RESPONSE_METADATA: &str = "SdkResponseMetadata";

/// The header an AWS service names the request it answered in, which the SDK reports
/// as the response's request id.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// How many milliseconds one second holds, which is the whole of the conversion
/// between the wire form of a timestamp and the SDK's.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// The largest figure of milliseconds this conversion reports: 2^53, the last integer
/// a double holds exactly, so a value past it is left as it came back rather than
/// reported as an integer it is not. It is some 285 000 years from the epoch, far past
/// any instant a service answers.
const MAX_EPOCH_MILLIS: f64 = 9_007_199_254_740_992.0;

/// `document` with each of `members` reported in whole epoch MILLISECONDS.
///
/// An `awsJson` timestamp is on the wire as epoch SECONDS with a fractional part,
/// while the SDK response a `Task` state reads renders a date as the integer number of
/// milliseconds; the instant is the same one either way, so the value is the answer's
/// own and only its unit changes. A member the answer did not carry, or carried as
/// something other than a number this can convert, is left exactly as it came back.
///
/// Pure, so the conversion is pinned by this module's own tests.
fn in_epoch_millis(document: Value, members: &[&str]) -> Value {
    if members.is_empty() {
        return document;
    }
    let Value::Object(mut carried) = document else {
        return document;
    };
    for member in members {
        // The member is read under both spellings a response may carry it in, as the
        // request's own members are ([`MemberCase`]).
        for spelling in [
            MemberCase::LowerCamel.spell(member),
            MemberCase::Pascal.spell(member),
        ] {
            let Some(seconds) = carried.get(&spelling).and_then(Value::as_f64) else {
                continue;
            };
            let millis = (seconds * MILLIS_PER_SECOND).round();
            if millis.is_finite() && millis.abs() < MAX_EPOCH_MILLIS {
                carried.insert(spelling, Value::Number((millis as i64).into()));
            }
        }
    }
    Value::Object(carried)
}

/// `document` as a `Task` state reports the literal SDK response of the call that
/// answered it: the operation's own members, plus the envelope the SDK puts the
/// response's HTTP facts in.
///
/// Every value in it is the ANSWER's own — the status the call came back with, and the
/// headers it carried, the request id among them — so a definition reading
/// `$.SdkHttpMetadata.HttpStatusCode` or `$.SdkResponseMetadata.RequestId` reads what
/// the call really answered, and a fact the answer did not carry is reported by
/// nothing rather than filled in: the envelope's members are there because the
/// integration returns them, and each holds exactly what came back. A response whose
/// body is not an object carries no members to put the envelope beside, so it is
/// reported as it stands.
///
/// Pure, so the shape of the envelope is pinned by this module's own tests.
fn with_sdk_envelope(document: Value, answered: &Answered) -> Value {
    let Value::Object(mut members) = document else {
        return document;
    };
    // `HttpHeaders` carries one value per header and `AllHttpHeaders` every value a
    // header was repeated with, which is the pair the SDK exposes.
    let mut single = Map::new();
    let mut all: Map<String, Value> = Map::new();
    for (name, value) in &answered.headers {
        single.insert(name.clone(), Value::String(value.clone()));
        match all.get_mut(name).and_then(Value::as_array_mut) {
            Some(values) => values.push(Value::String(value.clone())),
            None => {
                all.insert(name.clone(), Value::Array(vec![Value::String(value.clone())]));
            }
        }
    }
    let mut http = Map::new();
    if !single.is_empty() {
        http.insert("AllHttpHeaders".to_string(), Value::Object(all));
        http.insert("HttpHeaders".to_string(), Value::Object(single));
    }
    http.insert(
        "HttpStatusCode".to_string(),
        Value::Number(answered.status.into()),
    );
    members.insert(SDK_HTTP_METADATA.to_string(), Value::Object(http));
    let mut response = Map::new();
    if let Some(request_id) = answered.header(REQUEST_ID_HEADER) {
        response.insert("RequestId".to_string(), Value::String(request_id));
    }
    members.insert(SDK_RESPONSE_METADATA.to_string(), Value::Object(response));
    Value::Object(members)
}

/// The result or the failure one answer carries.
///
/// A 2xx answers the response document, which IS the state's result; an operation
/// whose output carries no member answers an empty body, which is the empty
/// document. Anything else is a refusal, reported as the error name Step Functions
/// gives a failed service call — the service's own id and the error code it
/// answered — so a definition's `Retry` and `Catch` match on the service's error.
/// A refusal with no code at all is left as ASL's own [`TASK_FAILED_ERROR`].
///
/// Pure, so the reading of every answer form is pinned by this module's own tests.
fn answer_of(
    api: &Api,
    operation: &str,
    status: u16,
    error_type: Option<&str>,
    payload: &[u8],
) -> Answer {
    let document: Option<Value> = if payload.is_empty() {
        None
    } else {
        ::serde_json::from_slice(payload).ok()
    };
    if (200..300).contains(&status) {
        return Ok(document.unwrap_or_else(|| Value::Object(Map::new())));
    }
    let code = error_code(error_type, document.as_ref());
    let message = document
        .as_ref()
        .and_then(error_message)
        .unwrap_or_default();
    // A gap in this emulator is not the service refusing the call: nothing was run
    // for the task, which is what it reports.
    if code.as_deref() == Some(NOT_IMPLEMENTED_CODE) {
        return Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "the '{operation}' operation of {} is not implemented by this emulator, so \
                 nothing was run for this task",
                api.sdk_id
            ),
        });
    }
    let (error, cause) = match code {
        Some(code) => (
            format!("{}.{code}", api.sdk_id),
            if message.is_empty() {
                format!("{} refused the '{operation}' call with {code}", api.sdk_id)
            } else {
                message
            },
        ),
        None => (
            TASK_FAILED_ERROR.to_string(),
            format!(
                "{} answered the '{operation}' call with HTTP {status} and no error code",
                api.sdk_id
            ),
        ),
    };
    Err(Failed { error, cause })
}

/// The error code an AWS JSON refusal carries: the `X-Amzn-Errortype` header, else
/// the body's `__type` discriminator, else its `code` member.
///
/// Each of those spellings may carry decoration an error NAME does not — a shape
/// id's namespace before `#`, a URL after `:` — so the code is the bare shape name
/// between them.
fn error_code(error_type: Option<&str>, document: Option<&Value>) -> Option<String> {
    let discriminator = error_type.map(str::to_string).or_else(|| {
        let document = document?;
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| document.get(*member).and_then(Value::as_str))
            .map(str::to_string)
    })?;
    let named = discriminator
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .trim();
    (!named.is_empty()).then(|| named.to_string())
}

/// The message an AWS JSON refusal carries, under either of the member spellings the
/// protocol allows.
fn error_message(document: &Value) -> Option<String> {
    ["message", "Message"]
        .iter()
        .find_map(|member| document.get(*member).and_then(Value::as_str))
        .map(str::to_string)
}

/// Sign one request with the emulator's own credentials and hand it to the service
/// its credential scope names, in process.
///
/// It is signed because the emulator verifies every request signature; the
/// credential scope is also what routes it, so the service that owns the operation
/// is the one that serves the call. The answer is its status, the error type it
/// named in the header, and its body.
async fn dispatch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: Vec<u8>,
) -> Result<Answered, String> {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", api.host(region))
        .header("content-type", api.content_type())
        .header(TARGET_HEADER, format!("{}.{operation}", api.target_prefix))
        .body(body)
        .map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = credentials(callback).await?;
    sign(
        &mut request,
        api.signing_name,
        region,
        &access_key_id,
        &secret_access_key,
    )?;

    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| error.to_string())?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string);
    // Every header the answer carried, in the order it carried them: the task's own
    // result reports them for an integration whose result is the literal SDK response
    // ([`with_sdk_envelope`]), so they are read off the answer rather than invented.
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok(Answered {
        status,
        error_type,
        headers,
        payload,
    })
}

/// What one in-process call came back with.
struct Answered {
    /// The HTTP status the service answered.
    status: u16,
    /// The error shape it named in [`ERROR_TYPE_HEADER`], for a refusal.
    error_type: Option<String>,
    /// Every header of the answer, as it carried them.
    headers: Vec<(String, String)>,
    /// Its body.
    payload: Vec<u8>,
}

impl Answered {
    /// The first value the answer carried `name` under, compared without regard to
    /// case as HTTP field names are.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(carried, _)| carried.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

/// The credentials a callback signs with: the emulator's own, through the provider
/// the framework hands every handler — which falls back to its placeholder pair when
/// the router vends none (request authentication off, where nothing verifies a
/// signature). They sign this call and are never reported or logged.
async fn credentials(
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = callback
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        credentials.access_key_id().to_string(),
        credentials.secret_access_key().to_string(),
    ))
}

/// SigV4-sign one in-process request to another emulated service, in place.
///
/// The settings are the ones the emulator's own verifier re-signs with
/// (`vendor/server/src/request_authentication.rs`), so the signature this produces
/// is the one that verifier computes over the same request — which is what lets a
/// task's call be served with request authentication ON.
///
/// TO LIFT: this, [`dispatch`] and [`credentials`] are the SECOND copy of the
/// signed in-process callback glue in this tree; the first is
/// `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs`
/// (`sign_callback_request`, `dispatch`, `callback_credentials`), whose settings
/// must stay identical to the verifier's. Their shared home is the vended framework
/// beside that verifier, which cannot be edited from here, and `services/common`
/// would put the signer on every service in the workspace. Whoever lifts it takes
/// BOTH copies and CloudFormation's `tests/all/callback_signature.rs`, which pins
/// the settings, together — and a third caller must lift rather than copy again.
fn sign(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Result<(), String> {
    use ::aws_sigv4::http_request::{
        sign as sign_request, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
    };

    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name("aws-emulator-sfn")
            .access_key_id(access_key_id)
            .secret_access_key(secret_access_key)
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let signable_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        signable_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign_request(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the `Resource` table: the case, the `Resource`, the service's
    /// signing name and the operation it names when a call is made for it, and
    /// whether that call STARTS a job the task is parked at (the run-a-job pattern).
    type ResourceRow = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
        bool,
    );

    /// One row of the answer table: the case, the status, the error-type header, the
    /// body, and the result document or the error name expected of it.
    type AnswerRow = (
        &'static str,
        u16,
        Option<&'static str>,
        &'static str,
        Result<&'static str, &'static str>,
    );

    /// One row of the access-request table: the case, the `Resource`, the request the
    /// state built, and the `(action, resource)` pair the call is subject to.
    type AccessRow = (
        &'static str,
        &'static str,
        Value,
        Option<(&'static str, String)>,
    );

    /// One row of the envelope table: the case, the document the operation answered,
    /// and the headers its answer carried.
    type EnvelopeRow = (&'static str, Value, Vec<(&'static str, &'static str)>);

    /// One row of the instant table: the case, the members to convert, the answer and
    /// the result reported for it.
    type InstantRow = (&'static str, &'static [&'static str], Value, Value);

    /// Which call a `Resource` names, over a TABLE of the forms a definition writes
    /// one in: the two integration families, the services in [`APIS`] and the forms
    /// that name no call this emulator makes.
    #[test]
    fn a_resource_names_the_call_its_action_does() {
        // (case, the Resource, the signing name and operation it names, parks at a job)
        let rows: [ResourceRow; 22] = [
            (
                "the optimized DynamoDB integration",
                "arn:aws:states:::dynamodb:putItem",
                Some(("dynamodb", "PutItem")),
                false,
            ),
            (
                "another action of the same service, which no code here names",
                "arn:aws:states:::dynamodb:updateItem",
                Some(("dynamodb", "UpdateItem")),
                false,
            ),
            (
                "the AWS SDK integration of the same operation",
                "arn:aws:states:::aws-sdk:dynamodb:putItem",
                Some(("dynamodb", "PutItem")),
                false,
            ),
            (
                "an AWS SDK integration whose service's SDK name is not its ARN namespace",
                "arn:aws:states:::aws-sdk:sfn:listStateMachines",
                Some(("states", "ListStateMachines")),
                false,
            ),
            (
                "an AWS SDK integration with EventBridge, whose SDK name is not 'events'",
                "arn:aws:states:::aws-sdk:eventbridge:putEvents",
                Some(("events", "PutEvents")),
                false,
            ),
            (
                "an ARN namespace the AWS SDK family does not name a service by",
                "arn:aws:states:::aws-sdk:states:listStateMachines",
                None,
                false,
            ),
            (
                "an action whose name is one word",
                "arn:aws:states:::aws-sdk:sqs:receiveMessage",
                Some(("sqs", "ReceiveMessage")),
                false,
            ),
            (
                "an EventBridge action",
                "arn:aws:states:::events:putEvents",
                Some(("events", "PutEvents")),
                false,
            ),
            (
                "a nested execution",
                "arn:aws:states:::states:startExecution",
                Some(("states", "StartExecution")),
                false,
            ),
            (
                "a Kinesis action",
                "arn:aws:states:::aws-sdk:kinesis:putRecord",
                Some(("kinesis", "PutRecord")),
                false,
            ),
            (
                "a Systems Manager action",
                "arn:aws:states:::aws-sdk:ssm:getParameter",
                Some(("ssm", "GetParameter")),
                false,
            ),
            (
                "an ECS action",
                "arn:aws:states:::ecs:runTask",
                Some(("ecs", "RunTask")),
                false,
            ),
            // The run-a-job pattern: the ACTION is what is called, and the task is
            // parked at the job that call starts.
            (
                "the run-a-job pattern over a nested execution",
                "arn:aws:states:::states:startExecution.sync",
                Some(("states", "StartExecution")),
                true,
            ),
            (
                "its second revision",
                "arn:aws:states:::states:startExecution.sync:2",
                Some(("states", "StartExecution")),
                true,
            ),
            (
                "the run-a-job pattern over an Athena query",
                "arn:aws:states:::athena:startQueryExecution.sync",
                Some(("athena", "StartQueryExecution")),
                true,
            ),
            // The forms that name no call.
            (
                "a service whose request is not one JSON document",
                "arn:aws:states:::sns:publish",
                None,
                false,
            ),
            (
                "the run-a-job pattern over an integration whose job cannot be reported",
                "arn:aws:states:::dynamodb:putItem.sync",
                None,
                false,
            ),
            (
                "the .waitForTaskToken pattern",
                "arn:aws:states:::sqs:sendMessage.waitForTaskToken",
                None,
                false,
            ),
            (
                "a pattern this service does not name",
                "arn:aws:states:::states:startExecution.whenever",
                None,
                false,
            ),
            (
                "an activity, whose work is a worker's rather than a call",
                "arn:aws:states:us-west-2:000000000000:activity:worker",
                None,
                false,
            ),
            (
                "a bare resource ARN",
                "arn:aws:lambda:us-west-2:000000000000:function:f",
                None,
                false,
            ),
            ("text that is not an ARN", "putItem", None, false),
        ];

        for (case, resource, expected, parks) in rows {
            match (read(resource), expected) {
                (Ok(call), Some((signing_name, operation))) => {
                    assert_eq!(
                        call.api.signing_name, signing_name,
                        "case {case:?}: expected the service {signing_name}, got {}",
                        call.api.signing_name
                    );
                    assert_eq!(
                        call.operation, operation,
                        "case {case:?}: expected the operation {operation}, got {}",
                        call.operation
                    );
                    assert_eq!(
                        call.job.is_some(),
                        parks,
                        "case {case:?}: expected parks-at-a-job {parks}, got {:?}",
                        call.job.map(|job| job.starts)
                    );
                }
                (Err(why), None) => assert!(
                    why.contains("nothing was run for it"),
                    "case {case:?}: a resource no call is made for must say nothing ran, got \
                     {why:?}"
                ),
                (Ok(call), None) => panic!(
                    "case {case:?}: expected no call, got {}.{}",
                    call.api.signing_name, call.operation
                ),
                (Err(why), Some(expected)) => {
                    panic!("case {case:?}: expected {expected:?}, got the refusal {why:?}")
                }
            }
        }
    }

    /// What one answer means for the task, over a TABLE spanning the answer forms a
    /// JSON service gives: a document, an empty success, and a refusal in each of
    /// the spellings its error code may arrive in.
    #[test]
    fn an_answer_is_the_result_or_the_error_the_service_named() {
        let api = APIS
            .iter()
            .find(|api| api.namespace == "dynamodb")
            .expect("DynamoDB is a service a task calls");
        // (case, status, the error-type header, the body, the expected answer)
        let rows: [AnswerRow; 8] = [
            (
                "a response document is the state's result",
                200,
                None,
                r#"{"Attributes":{"pk":{"S":"a"}}}"#,
                Ok(r#"{"Attributes":{"pk":{"S":"a"}}}"#),
            ),
            (
                "an operation whose output carries nothing answers the empty document",
                200,
                None,
                "",
                Ok("{}"),
            ),
            (
                "an empty JSON body is itself the empty document",
                200,
                None,
                "{}",
                Ok("{}"),
            ),
            (
                "the error type names the error",
                400,
                Some("ConditionalCheckFailedException"),
                r#"{"message":"the conditional request failed"}"#,
                Err("DynamoDB.ConditionalCheckFailedException"),
            ),
            (
                "a header carrying a shape id and a URL",
                400,
                Some("com.amazonaws.dynamodb#ResourceNotFoundException:http://internal"),
                r#"{"message":"no such table"}"#,
                Err("DynamoDB.ResourceNotFoundException"),
            ),
            (
                "the body's __type when no header names it",
                400,
                None,
                r#"{"__type":"com.amazonaws.dynamodb#ValidationException","message":"bad"}"#,
                Err("DynamoDB.ValidationException"),
            ),
            (
                "a refusal naming no code at all",
                500,
                None,
                "",
                Err(TASK_FAILED_ERROR),
            ),
            (
                "a gap in this emulator is not a refusal of the call",
                501,
                Some("NotImplemented"),
                r#"{"message":"not implemented"}"#,
                Err(super::super::asl::RUNTIME_ERROR),
            ),
        ];

        for (case, status, error_type, body, expected) in rows {
            let answer = answer_of(api, "PutItem", status, error_type, body.as_bytes());
            match (&answer, expected) {
                (Ok(result), Ok(document)) => {
                    let expected: Value =
                        ::serde_json::from_str(document).expect("the expected result is JSON");
                    assert_eq!(
                        *result, expected,
                        "case {case:?}: expected the result {expected}, got {result}"
                    );
                }
                (Err(failed), Err(error)) => {
                    assert_eq!(
                        failed.error, error,
                        "case {case:?}: expected the error {error}, got {failed:?}"
                    );
                    assert!(
                        !failed.cause.is_empty(),
                        "case {case:?}: a failed task must say what happened, got {failed:?}"
                    );
                }
                _ => panic!("case {case:?}: expected {expected:?}, got {answer:?}"),
            }
        }
    }

    /// What one request looks like on the wire, over a TABLE spanning the two
    /// member conventions a model spells its input in, both spellings a definition
    /// may write a member under, the nested data that is NOT re-spelled, and the
    /// member an integration documents as a document while the operation types it
    /// as a string.
    #[test]
    fn a_request_carries_each_member_as_the_invoked_model_spells_it() {
        // (case, the Resource, the document the state built, the document sent)
        let rows: [(&str, &str, Value, Value); 6] = [
            (
                "a lowerCamel model's member, written in the integration's PascalCase",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"StateMachineArn": "arn:sm", "Name": "run"}),
                ::serde_json::json!({"stateMachineArn": "arn:sm", "name": "run"}),
            ),
            (
                "the same member already written as that model spells it",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"stateMachineArn": "arn:sm"}),
                ::serde_json::json!({"stateMachineArn": "arn:sm"}),
            ),
            (
                "a PascalCase model's members, which the convention leaves alone",
                "arn:aws:states:::dynamodb:putItem",
                ::serde_json::json!({"TableName": "t", "Item": {"pk": {"S": "a"}}}),
                ::serde_json::json!({"TableName": "t", "Item": {"pk": {"S": "a"}}}),
            ),
            (
                "nested data is a caller's own, so no key below the top level is re-spelled",
                "arn:aws:states:::aws-sdk:ecs:runTask",
                ::serde_json::json!({"TaskDefinition": "d", "Overrides": {"ContainerOverrides": []}}),
                ::serde_json::json!({"taskDefinition": "d", "overrides": {"ContainerOverrides": []}}),
            ),
            (
                "the child execution's input, which the operation types as a string",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"StateMachineArn": "arn:sm", "Input": {"a": [1, 2]}}),
                ::serde_json::json!({"stateMachineArn": "arn:sm", "input": r#"{"a":[1,2]}"#}),
            ),
            (
                "that input already written as the text the operation types",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"Input": "{\"a\":1}"}),
                ::serde_json::json!({"input": "{\"a\":1}"}),
            ),
        ];

        for (case, resource, built, expected) in rows {
            let call = read(resource).expect("every row names a call");
            let sent = wire_request(&call, &built);
            assert_eq!(
                sent, expected,
                "case {case:?}: expected the request {expected}, got {sent}"
            );
        }
    }

    /// An AWS SDK integration converts nothing: its parameters ARE the invoked
    /// operation's own members, so the member the optimized integration takes as a
    /// document is the operation's string there.
    #[test]
    fn an_aws_sdk_integration_carries_the_operations_own_members() {
        let call = read("arn:aws:states:::aws-sdk:sfn:startExecution").expect("this names a call");
        let sent = wire_request(&call, &::serde_json::json!({"Input": "{\"a\":1}"}));
        assert_eq!(
            sent,
            ::serde_json::json!({"input": "{\"a\":1}"}),
            "case \"the AWS SDK family\": expected the operation's own members, got {sent}"
        );
        assert!(
            call.json_text.is_empty(),
            "case \"the AWS SDK family\": nothing is converted for one, got {:?}",
            call.json_text
        );
    }

    /// A chain of in-process calls already at [`MAX_CALL_CHAIN_DEPTH`] makes NO
    /// further call: the depth decides BEFORE the handover, so the blocking thread
    /// this walk holds is the last one the chain pins, and the task reports that
    /// nothing was run rather than a result nothing produced.
    ///
    /// The rows are the depths at and past the ceiling; every call the end-to-end
    /// suites make runs below it, which is the other side of the same rule.
    #[test]
    fn a_call_from_a_chain_at_the_depth_ceiling_is_not_made() {
        let call = read("arn:aws:states:::dynamodb:putItem").expect("this names a call");
        let (calls, mut handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
        let rows: [(&str, u8); 2] = [
            ("a chain at the ceiling", MAX_CALL_CHAIN_DEPTH),
            ("a chain past the ceiling", MAX_CALL_CHAIN_DEPTH + 1),
        ];

        for (case, depth) in rows {
            let dispatcher = Dispatcher {
                calls: calls.clone(),
                depth,
                role: None,
            };
            let answer = dispatcher.perform(&call, &::serde_json::json!({}), None);
            let Err(failed) = answer else {
                panic!("case {case:?}: expected no call to be made, got {answer:?}")
            };
            assert_eq!(
                failed.error,
                super::super::asl::RUNTIME_ERROR,
                "case {case:?}: expected {}, got {failed:?}",
                super::super::asl::RUNTIME_ERROR
            );
            assert!(
                failed.cause.contains("nothing was run for it"),
                "case {case:?}: the cause must say nothing ran, got {failed:?}"
            );
            assert!(
                handovers.try_recv().is_err(),
                "case {case:?}: no call may be handed over at this depth"
            );
        }
    }

    /// A call that does not answer inside the span the task gave it times the task
    /// out and RELEASES the thread it was waiting on, so a state's
    /// `TimeoutSeconds` bounds the span it documents rather than only being
    /// compared with it afterwards.
    ///
    /// The handover is never served here, which is what a stalled call looks like
    /// from the waiting side.
    #[test]
    fn a_call_that_does_not_answer_inside_its_bound_times_the_task_out() {
        let call = read("arn:aws:states:::dynamodb:putItem").expect("this names a call");
        // The serving half is held, never drained: the send succeeds and no answer
        // ever comes back.
        let (calls, _handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
        let dispatcher = Dispatcher {
            calls,
            depth: 0,
            role: None,
        };
        let within = Duration::from_millis(50);

        let began = ::std::time::Instant::now();
        let answer = dispatcher.perform(&call, &::serde_json::json!({}), Some(within));
        let waited = began.elapsed();

        let Err(failed) = answer else {
            panic!("case \"a call that never answers\": expected a failure, got {answer:?}")
        };
        assert_eq!(
            failed.error,
            super::super::asl::TIMEOUT_ERROR,
            "case \"a call that never answers\": expected {}, got {failed:?}",
            super::super::asl::TIMEOUT_ERROR
        );
        assert!(
            waited >= within,
            "case \"a call that never answers\": the task must wait the span it was given, \
             waited {waited:?}"
        );
        assert!(
            waited < MAX_CALL_DURATION,
            "case \"a call that never answers\": the state's own bound must decide, not the \
             {MAX_CALL_DURATION:?} ceiling, waited {waited:?}"
        );
    }

    /// The result of a request-response OPTIMIZED integration is the literal SDK
    /// response of the call: the operation's own members, plus the envelope the SDK
    /// puts that response's HTTP facts in, with every value read off the answer.
    ///
    /// The rows span what the rule generalises over: a response with members and one
    /// with none, a repeated header, an answer carrying no request id at all, and a
    /// body that is not an object.
    #[test]
    fn an_optimized_integrations_result_carries_the_sdk_response_envelope() {
        // (case, the document the operation answered, the headers it carried)
        let rows: [EnvelopeRow; 4] = [
            (
                "a response with members of its own",
                ::serde_json::json!({"ExecutionArn": "arn:execution", "StartDate": 1}),
                vec![("x-amzn-requestid", "req-1"), ("content-type", "text/json")],
            ),
            (
                "a response carrying no member at all",
                ::serde_json::json!({}),
                vec![("x-amzn-RequestId", "req-2")],
            ),
            (
                "a header the answer repeated",
                ::serde_json::json!({"Attributes": {}}),
                vec![
                    ("x-amzn-requestid", "req-3"),
                    ("set-cookie", "a=1"),
                    ("set-cookie", "b=2"),
                ],
            ),
            (
                "an answer naming no request id",
                ::serde_json::json!({"Item": {}}),
                vec![("content-type", "text/json")],
            ),
        ];

        for (case, document, headers) in rows {
            let answered = Answered {
                status: 200,
                error_type: None,
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                payload: Vec::new(),
            };
            let members = document.as_object().expect("each row answers a document").clone();
            let result = with_sdk_envelope(document, &answered);
            // Every member the operation answered is still there, untouched.
            for (member, value) in &members {
                assert_eq!(
                    result.get(member),
                    Some(value),
                    "case {case:?}: expected the operation's own {member:?}, got {result}"
                );
            }
            assert_eq!(
                result.pointer("/SdkHttpMetadata/HttpStatusCode"),
                Some(&::serde_json::json!(200)),
                "case {case:?}: expected the status the call answered, got {result}"
            );
            for (name, value) in &headers {
                assert!(
                    result
                        .pointer(&format!("/SdkHttpMetadata/AllHttpHeaders/{name}"))
                        .and_then(Value::as_array)
                        .is_some_and(|carried| carried.contains(&::serde_json::json!(value))),
                    "case {case:?}: expected the header {name:?} among the answer's, got {result}"
                );
            }
            let request_id = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(REQUEST_ID_HEADER))
                .map(|(_, value)| ::serde_json::json!(value));
            assert_eq!(
                result.pointer("/SdkResponseMetadata/RequestId"),
                request_id.as_ref(),
                "case {case:?}: expected the request id the answer named, got {result}"
            );
        }

        // A body that is not an object carries no members to put the envelope beside.
        let answered = Answered {
            status: 200,
            error_type: None,
            headers: Vec::new(),
            payload: Vec::new(),
        };
        let listed = with_sdk_envelope(::serde_json::json!([1, 2]), &answered);
        assert_eq!(
            listed,
            ::serde_json::json!([1, 2]),
            "case \"a response that is not an object\": expected it as it stands, got {listed}"
        );
    }

    /// The access request a call is subject to: the IAM action the invoked operation
    /// publishes, and the ARN the request names as the resource acted on.
    /// The rows span what the rule generalises over: both spellings a definition may
    /// write the member in, the pattern suffixes (which name the same action), a
    /// member that names its resource by NAME rather than by ARN (and the same member
    /// carrying an ARN instead), several actions of one service, an integration the
    /// table carries no entry for, the AWS SDK family, and a request naming no
    /// resource at all.
    #[test]
    fn an_access_request_is_the_action_and_the_resource_the_request_names() {
        let child = "arn:aws:states:us-west-2:000000000000:stateMachine:child";
        let region = "eu-west-1";
        let orders = ::engine::emulator_arn("dynamodb", region, "table/orders");
        let elsewhere = "arn:aws:dynamodb:us-east-2:000000000000:table/elsewhere";
        // (case, the Resource, the request the state built, the expected pair)
        let rows: [AccessRow; 10] = [
            (
                "a nested execution, the member in the integration's own spelling",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"StateMachineArn": child}),
                Some(("states:StartExecution", child.to_string())),
            ),
            (
                "the same member as the invoked model spells it",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"stateMachineArn": child}),
                Some(("states:StartExecution", child.to_string())),
            ),
            (
                "the run-a-job pattern, whose action is the same one",
                "arn:aws:states:::states:startExecution.sync:2",
                ::serde_json::json!({"StateMachineArn": child, "Input": {}}),
                Some(("states:StartExecution", child.to_string())),
            ),
            (
                "a member naming its table by name, which is the ARN of that table here",
                "arn:aws:states:::dynamodb:putItem",
                ::serde_json::json!({"TableName": "orders"}),
                Some(("dynamodb:PutItem", orders.clone())),
            ),
            (
                "another action of the same service, over the same table",
                "arn:aws:states:::dynamodb:deleteItem",
                ::serde_json::json!({"TableName": "orders"}),
                Some(("dynamodb:DeleteItem", orders.clone())),
            ),
            (
                "the AWS SDK family of that call, whose member its model spells lower",
                "arn:aws:states:::aws-sdk:dynamodb:updateItem",
                ::serde_json::json!({"tableName": "orders"}),
                Some(("dynamodb:UpdateItem", orders.clone())),
            ),
            (
                "the same member carrying an ARN, which names the table it names",
                "arn:aws:states:::dynamodb:getItem",
                ::serde_json::json!({"TableName": elsewhere}),
                Some(("dynamodb:GetItem", elsewhere.to_string())),
            ),
            (
                "an integration no entry is carried for",
                "arn:aws:states:::sqs:sendMessage",
                ::serde_json::json!({"QueueUrl": "https://sqs.eu-west-1.amazonaws.com/0/q"}),
                None,
            ),
            (
                "the AWS SDK family of a nested execution, which acts on the same resource",
                "arn:aws:states:::aws-sdk:sfn:startExecution",
                ::serde_json::json!({"stateMachineArn": child}),
                Some(("states:StartExecution", child.to_string())),
            ),
            (
                "a request naming no resource at all",
                "arn:aws:states:::states:startExecution",
                ::serde_json::json!({"Input": {}}),
                None,
            ),
        ];

        for (case, resource, request, expected) in rows {
            let call = read(resource).expect("every row names a call");
            let asked = access_request(&call, &request, region);
            let expected = expected.map(|(action, named)| (action.to_string(), named));
            assert_eq!(
                asked, expected,
                "case {case:?}: expected {expected:?}, got {asked:?}"
            );
        }
    }

    /// An instant the invoked operation answered is reported in whole epoch
    /// MILLISECONDS, which is the unit the SDK response a `Task` state reads renders a
    /// date in — the `awsJson` wire form is epoch seconds with a fractional part.
    ///
    /// The rows span what the rule generalises over: a fractional figure, a whole one,
    /// the epoch itself, both spellings the member may come back under, a member the
    /// response does not carry, one carrying something that is not a number, and a
    /// member no entry names.
    #[test]
    fn an_instant_in_a_result_is_reported_in_epoch_millis() {
        // (case, the members to convert, the answer, the result)
        let rows: [InstantRow; 7] = [
            (
                "a timestamp carrying a fractional second",
                &["startDate"],
                ::serde_json::json!({"executionArn": "arn:e", "startDate": 1_793_241_840.725}),
                ::serde_json::json!({"executionArn": "arn:e", "startDate": 1_793_241_840_725i64}),
            ),
            (
                "a whole number of seconds",
                &["startDate"],
                ::serde_json::json!({"startDate": 1_793_241_840i64}),
                ::serde_json::json!({"startDate": 1_793_241_840_000i64}),
            ),
            (
                "the epoch itself",
                &["startDate"],
                ::serde_json::json!({"startDate": 0}),
                ::serde_json::json!({"startDate": 0}),
            ),
            (
                "the member under the integration's own spelling",
                &["startDate"],
                ::serde_json::json!({"StartDate": 1.5}),
                ::serde_json::json!({"StartDate": 1_500i64}),
            ),
            (
                "a member the response does not carry",
                &["startDate"],
                ::serde_json::json!({"executionArn": "arn:e"}),
                ::serde_json::json!({"executionArn": "arn:e"}),
            ),
            (
                "a member carrying something that is not a number",
                &["startDate"],
                ::serde_json::json!({"startDate": "2026-10-28T20:13:41Z"}),
                ::serde_json::json!({"startDate": "2026-10-28T20:13:41Z"}),
            ),
            (
                "an integration whose entry names no member",
                &[],
                ::serde_json::json!({"startDate": 1.5}),
                ::serde_json::json!({"startDate": 1.5}),
            ),
        ];

        for (case, members, answer, expected) in rows {
            let reported = in_epoch_millis(answer, members);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected}, got {reported}"
            );
        }

        // The catalogue names the member for the integration whose result carries an
        // instant, so the rule reaches a task through `read` and not only through the
        // conversion above.
        let call = read("arn:aws:states:::states:startExecution").expect("this names a call");
        assert!(
            call.epoch_millis.contains(&"startDate"),
            "case \"a nested execution\": expected the start date among the instants its result \
             reports, got {:?}",
            call.epoch_millis
        );
    }
}
