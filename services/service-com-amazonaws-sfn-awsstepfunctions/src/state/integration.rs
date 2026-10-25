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
//! A `Resource` of the form `arn:aws:states:::<service>:<action>` (an optimized
//! integration) or `arn:aws:states:::aws-sdk:<service>:<action>` (an AWS SDK
//! integration) names a service by its ARN namespace and one of that service's API
//! operations by its name in lowerCamel. The call is made when that service has an
//! [`APIS`] entry — the services whose protocol carries the request as ONE JSON
//! document, the `awsJson1_0` and `awsJson1_1` protocols, whose body IS the
//! operation's input document, which is exactly what a payload template builds. One
//! entry per service, and EVERY operation of a listed service is reachable through
//! it: the operation name is derived from the action, so nothing here is written per
//! action.
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
//! * an integration pattern that waits for something this call cannot wait for:
//!   `.sync`, which polls the invoked job to completion, and `.waitForTaskToken`,
//!   which needs a later `SendTaskSuccess` from outside the execution;
//! * a bare resource ARN (a Lambda function ARN as the `Resource`), which names a
//!   resource rather than an integration call.
//!
//! An ACTIVITY `Resource` is not one of these: its work belongs to a worker that
//! polls this emulator, so the walk schedules a task and suspends at it rather than
//! asking for a call ([`super::asl::Interpreter`] decides that before [`read`] is
//! asked anything). [`read`] still answers for one, so it is total over every
//! `Resource` form a definition can carry.
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
    /// error ([`Failed::error`]).
    pub sdk_id: &'static str,
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
    },
];

/// One integration call: the service's API, and the operation of it the `Resource`'s
/// action named.
#[derive(Debug, Clone)]
pub struct Call {
    pub api: &'static Api,
    /// The operation name as the service's model spells it, derived from the action.
    pub operation: String,
}

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
    let (service, action) = match rest.split_once(':') {
        Some((AWS_SDK_SEGMENT, tail)) => tail.split_once(':').ok_or_else(|| {
            format!("'{shown}' names no action of its service, so nothing was run for it")
        })?,
        Some(pair) => pair,
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
    // "<action>.sync", "<action>.sync:2" and "<action>.waitForTaskToken" each ask
    // for an outcome that arrives after this call: the invoked job's completion, or
    // a later `SendTaskSuccess`.
    if let Some((_action, pattern)) = action.split_once('.') {
        return Err(format!(
            "'{shown}' asks for the '{}' integration pattern, whose outcome arrives after the \
             call that starts it, so nothing was run for it",
            super::asl::echoed(pattern)
        ));
    }
    if action.is_empty() {
        return Err(format!(
            "'{shown}' names no action of its service, so nothing was run for it"
        ));
    }
    let api = APIS
        .iter()
        .find(|api| api.namespace == service)
        .ok_or_else(|| {
            format!(
                "'{shown}' names an integration with '{}', which is not one of the services this \
                 emulator makes a 'Task' state's integration call to, so nothing was run for it",
                super::asl::echoed(service)
            )
        })?;
    Ok(Call {
        api,
        operation: operation_of(action),
    })
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
}

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
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    let depth = chain_depth(ctx);
    // One call is in flight at a time: the walk waits for each answer before it
    // transitions, so a deeper channel would buffer nothing.
    let (calls, mut handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    let walk = ::tokio::task::spawn_blocking(move || {
        let dispatcher = Dispatcher { calls, depth };
        work(&dispatcher)
    });
    let serve = async {
        while let Some(handover) = handovers.recv().await {
            let answer = perform(ctx, region, &handover.call, &handover.request).await;
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
fn chain_depth(ctx: &smithy_server_runtime::OperationContext<'_>) -> u8 {
    ctx.parts()
        .extensions
        .get::<smithy_server_runtime::CallbackRecursionDepth>()
        .map_or(0, |depth| depth.0)
}

/// Make one integration call and read what the service answered.
async fn perform(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    call: &Call,
    request: &Value,
) -> Answer {
    let api = call.api;
    let body = match ::serde_json::to_vec(request) {
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
    match dispatch(ctx, api, region, &call.operation, body).await {
        Ok((status, error_type, payload)) => {
            answer_of(api, &call.operation, status, error_type.as_deref(), &payload)
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
    ctx: &smithy_server_runtime::OperationContext<'_>,
    api: &Api,
    region: &str,
    operation: &str,
    body: Vec<u8>,
) -> Result<(u16, Option<String>, Vec<u8>), String> {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", api.host(region))
        .header("content-type", api.content_type())
        .header(TARGET_HEADER, format!("{}.{operation}", api.target_prefix))
        .body(body)
        .map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = credentials(ctx).await?;
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
        &ctx.callback().http_connector(),
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string);
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok((status, error_type, payload))
}

/// The credentials a callback signs with: the emulator's own, through the provider
/// the framework hands every handler — which falls back to its placeholder pair when
/// the router vends none (request authentication off, where nothing verifies a
/// signature). They sign this call and are never reported or logged.
async fn credentials(
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = ctx
        .callback()
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

    /// One row of the `Resource` table: the case, the `Resource`, and the service's
    /// signing name and the operation it names when a call is made for it.
    type ResourceRow = (&'static str, &'static str, Option<(&'static str, &'static str)>);

    /// One row of the answer table: the case, the status, the error-type header, the
    /// body, and the result document or the error name expected of it.
    type AnswerRow = (
        &'static str,
        u16,
        Option<&'static str>,
        &'static str,
        Result<&'static str, &'static str>,
    );

    /// Which call a `Resource` names, over a TABLE of the forms a definition writes
    /// one in: the two integration families, the services in [`APIS`] and the forms
    /// that name no call this emulator makes.
    #[test]
    fn a_resource_names_the_call_its_action_does() {
        // (case, the Resource, the signing name and operation it names)
        let rows: [ResourceRow; 15] = [
            (
                "the optimized DynamoDB integration",
                "arn:aws:states:::dynamodb:putItem",
                Some(("dynamodb", "PutItem")),
            ),
            (
                "another action of the same service, which no code here names",
                "arn:aws:states:::dynamodb:updateItem",
                Some(("dynamodb", "UpdateItem")),
            ),
            (
                "the AWS SDK integration of the same operation",
                "arn:aws:states:::aws-sdk:dynamodb:putItem",
                Some(("dynamodb", "PutItem")),
            ),
            (
                "an action whose name is one word",
                "arn:aws:states:::aws-sdk:sqs:receiveMessage",
                Some(("sqs", "ReceiveMessage")),
            ),
            (
                "an EventBridge action",
                "arn:aws:states:::events:putEvents",
                Some(("events", "PutEvents")),
            ),
            (
                "a nested execution",
                "arn:aws:states:::states:startExecution",
                Some(("states", "StartExecution")),
            ),
            (
                "a Kinesis action",
                "arn:aws:states:::aws-sdk:kinesis:putRecord",
                Some(("kinesis", "PutRecord")),
            ),
            (
                "a Systems Manager action",
                "arn:aws:states:::aws-sdk:ssm:getParameter",
                Some(("ssm", "GetParameter")),
            ),
            (
                "an ECS action",
                "arn:aws:states:::ecs:runTask",
                Some(("ecs", "RunTask")),
            ),
            // The forms that name no call.
            (
                "a service whose request is not one JSON document",
                "arn:aws:states:::sns:publish",
                None,
            ),
            (
                "the .sync pattern",
                "arn:aws:states:::states:startExecution.sync",
                None,
            ),
            (
                "the .waitForTaskToken pattern",
                "arn:aws:states:::sqs:sendMessage.waitForTaskToken",
                None,
            ),
            (
                "an activity, whose work is a worker's rather than a call",
                "arn:aws:states:us-west-2:000000000000:activity:worker",
                None,
            ),
            (
                "a bare resource ARN",
                "arn:aws:lambda:us-west-2:000000000000:function:f",
                None,
            ),
            ("text that is not an ARN", "putItem", None),
        ];

        for (case, resource, expected) in rows {
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
        let dispatcher = Dispatcher { calls, depth: 0 };
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
}
