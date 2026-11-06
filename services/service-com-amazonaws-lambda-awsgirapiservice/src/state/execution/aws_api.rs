// SPDX-License-Identifier: CC0-1.0

//! The seam a RUNNING handler reaches the rest of this emulator through: one AWS API call,
//! made by the host on the guest's behalf.
//!
//! AWS Lambda's managed runtimes ship the AWS SDK for the handler's language on its import
//! path, so a handler can call another AWS service with no dependency of its own — and a
//! very large share of real functions exist to do exactly that. This module is the host half
//! of that: the guest hands over a service token, an operation name and the operation's
//! input document, and the host builds the HTTP request that service's own protocol
//! describes, signs it, and dispatches it through the in-process callback
//! ([`::smithy_server_runtime::EmbeddedEmulator`]) into the same emulator the function is
//! running inside. The answer is that service's own answer.
//!
//! What the guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator. The request is dispatched through the
//!   callback transport, which hands it to this process's router; no host name the guest
//!   supplies is resolved and no socket is opened, so no call can leave the process. The
//!   framework's own callback recursion limit bounds a chain of self-invocations.
//! * The guest never sees a credential. The signature is put on the request by host code
//!   with the credential the framework hands this emulator's handlers, exactly as every
//!   other in-process call between two emulated services is signed
//!   ([`::service_common::in_process_sigv4`]); nothing of it is returned to the guest, and
//!   the guest cannot ask for a call to be signed as anything else.
//! * Only the services in [`APIS`] are reachable, and only the operations their own models
//!   bind. A token that is not in that table is REFUSED rather than guessed at: the guest
//!   is told the service is unknown, and nothing is sent.
//! * Every bound is the host's. The size of the document the guest may send, the size of
//!   the answer it may read back and how long one call may take are all checked here and in
//!   [`super::python`], against the run's own remaining deadline.
//!
//! Nothing about a call is synthesised: a status, an error code, a message or a value the
//! guest reads back came from the emulated service that answered, and a call this module
//! cannot make is reported as not made.

use ::std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

/// The header an AWS JSON client names the operation it calls in
/// (`<target prefix>.<OperationName>`).
const TARGET_HEADER: &str = "x-amz-target";

/// The header an AWS answer names the error shape of a refusal in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The header an AWS answer carries the id of the request it answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The largest input document one call may carry.
///
/// A HOST bound, well above any operation's own input limit and far below what a run's
/// memory ceiling allows, so a guest cannot turn one invocation into an unbounded request.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// The largest answer one call may read back, for the same reason.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// How many calls one run may make.
///
/// A HOST bound: a handler loops as it likes, but the work it can push into the rest of the
/// emulator from one invocation is capped, so no single invocation can drive the process
/// without end. Well above what a handler processing one event batch needs.
pub const MAX_CALLS_PER_RUN: usize = 256;

/// How one service's API is reached on the wire: the facts of that service's own model that
/// a request to it is built from.
///
/// Every field is read from the target service's Smithy model with
/// `smithy_query(query="service_info")`, so this table states no wire detail of its own.
#[derive(Debug)]
pub struct Api {
    /// The token an AWS SDK client names this service by — the service's SDK id, lower
    /// cased with its spaces dropped, which is what `boto3.client("sqs")` is given.
    pub sdk_token: &'static str,
    /// Its SigV4 signing name (`sigv4Name`) — also the credential scope that routes the
    /// call to this service inside this emulator.
    pub signing_name: &'static str,
    /// The leading label of its endpoint host (`awsService.endpointPrefix`).
    pub endpoint_prefix: &'static str,
    /// The `X-Amz-Target` prefix a client names an operation of this service under: the
    /// service shape's own name, as the model spells it.
    pub target_prefix: &'static str,
    /// The AWS JSON protocol version its model declares (`awsJson1_0` -> `1.0`), which
    /// names the media type of the request body.
    pub json_version: &'static str,
}

impl Api {
    /// The host one of this service's regional endpoints answers on.
    fn host(&self, region: &str) -> String {
        format!("{}.{region}.amazonaws.com", self.endpoint_prefix)
    }

    /// The media type this service's protocol names for a request body.
    fn content_type(&self) -> String {
        format!("application/x-amz-json-{}", self.json_version)
    }
}

/// The services a running handler may call, and how a request to each is bound to HTTP.
///
/// One entry per SERVICE, never per action: every one of these binds its requests with an
/// AWS JSON protocol, where the request body IS the operation's input document and the
/// operation is named in a header, so every operation each of them models is reachable
/// through the one code path below and none is restated here.
///
/// It is a LIST rather than a rule because each entry's wire facts are that service's own
/// model's, which this crate cannot read, and because a service whose protocol is NOT an
/// AWS JSON one binds its input to a path, a query, headers and a payload per operation —
/// which this seam does not describe. A token with no entry is one no call is made for, and
/// [`AwsAccess::call`] says exactly that rather than sending a request whose shape it is
/// guessing at.
pub const APIS: &[Api] = &[
    // `services/service-com-amazonaws-sqs-amazonsqs/smithy-model/
    // com-amazonaws-sqs-amazonsqs.json`: awsJson1_0, sigv4Name `sqs`, endpointPrefix
    // `sqs`, sdkId `SQS`, service shape `AmazonSQS`.
    Api {
        sdk_token: "sqs",
        signing_name: "sqs",
        endpoint_prefix: "sqs",
        target_prefix: "AmazonSQS",
        json_version: "1.0",
    },
    // `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/
    // com-amazonaws-eventbridge-awsevents.json`: awsJson1_1, sigv4Name `events`,
    // endpointPrefix `events`, sdkId `EventBridge`, service shape `AWSEvents`.
    Api {
        sdk_token: "events",
        signing_name: "events",
        endpoint_prefix: "events",
        target_prefix: "AWSEvents",
        json_version: "1.1",
    },
    // The same service under the token its own SDK id gives it, which is the other name an
    // SDK client is built with (`boto3.client("eventbridge")`).
    Api {
        sdk_token: "eventbridge",
        signing_name: "events",
        endpoint_prefix: "events",
        target_prefix: "AWSEvents",
        json_version: "1.1",
    },
    // `services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json`: awsJson1_0, sigv4Name `dynamodb`,
    // endpointPrefix `dynamodb`, sdkId `DynamoDB`, service shape `DynamoDB_20120810`.
    Api {
        sdk_token: "dynamodb",
        signing_name: "dynamodb",
        endpoint_prefix: "dynamodb",
        target_prefix: "DynamoDB_20120810",
        json_version: "1.0",
    },
];

/// Every service token a call may name, in the order [`APIS`] lists them — what the guest
/// is told when it names one that is not there.
#[must_use]
pub fn reachable_services() -> Vec<&'static str> {
    APIS.iter().map(|api| api.sdk_token).collect()
}

/// What a running handler reaches the rest of this emulator through.
///
/// Built from the operation context of the call that started the run, so the callback it
/// holds is the one the framework handed that handler, and the Region is the one the
/// invocation was addressed to. It is `Clone` and `'static` because a run happens on a
/// thread of its own, past the lifetime of the request that started it.
#[derive(Debug, Clone)]
pub struct AwsAccess {
    /// The in-process callback every call is dispatched through.
    callback: EmbeddedEmulator,
    /// The async runtime the dispatch is driven on. A run's thread is not one of its
    /// workers, so a call is driven by blocking that thread on this handle rather than by
    /// making the interpreter asynchronous.
    runtime: ::tokio::runtime::Handle,
    /// The Region a call is made in unless the SDK client the guest built names another.
    region: String,
}

impl AwsAccess {
    /// The access a handler invoked by this request gets, or `None` where there is no async
    /// runtime to dispatch a call on — in which case a run simply has no SDK and says so.
    #[must_use]
    pub fn new(callback: EmbeddedEmulator, region: &str) -> Option<Self> {
        let runtime = ::tokio::runtime::Handle::try_current().ok()?;
        Some(Self {
            callback,
            runtime,
            region: region.to_string(),
        })
    }

    /// The Region a call with no Region of its own is made in.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Make ONE call: `operation` of `service`, carrying `body` as its input document.
    ///
    /// `region` is the Region the SDK client the guest built names, which is honoured when
    /// it is a Region name at all and otherwise replaced by the invocation's own — a
    /// guest-supplied value reaches the request's host and credential scope, so it is
    /// checked here rather than taken as given. `within` bounds the call: it is what the
    /// run has left of its deadline, so a call cannot outlast the invocation that made it.
    pub fn call(
        &self,
        service: &str,
        operation: &str,
        region: &str,
        body: &str,
        within: Duration,
    ) -> Result<Answer, Refusal> {
        let Some(api) = APIS.iter().find(|api| api.sdk_token == service) else {
            return Err(Refusal::UnknownService);
        };
        if !well_formed_operation(operation) {
            return Err(Refusal::UnknownOperation);
        }
        if body.len() > MAX_REQUEST_BYTES {
            return Err(Refusal::RequestTooLarge);
        }
        let region = if well_formed_region(region) {
            region
        } else {
            &self.region
        };
        let answered = self.runtime.block_on(async {
            ::tokio::time::timeout(within, dispatch(&self.callback, api, region, operation, body))
                .await
        });
        match answered {
            Ok(answer) => answer,
            Err(_elapsed) => Err(Refusal::TimedOut),
        }
    }
}

/// Why a call was not made at all.
///
/// Each of these is reported to the guest as the failure it is; none of them is answered
/// with a made-up success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The token names no service this seam reaches ([`APIS`]).
    UnknownService,
    /// The operation name is not one an AWS JSON target header can carry.
    UnknownOperation,
    /// The input document is larger than [`MAX_REQUEST_BYTES`].
    RequestTooLarge,
    /// The answer is larger than [`MAX_RESPONSE_BYTES`].
    ResponseTooLarge,
    /// The call did not finish inside what the run had left.
    TimedOut,
    /// The call could not be built, signed or dispatched; the string is why.
    NotMade(String),
}

/// What one call came back with: the emulated service's own answer.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The HTTP status it answered.
    pub status: u16,
    /// Its body, as text.
    pub body: String,
    /// The error shape it named in [`ERROR_TYPE_HEADER`], for a refusal.
    pub error_type: Option<String>,
    /// The id it gave the request it answered.
    pub request_id: Option<String>,
}

/// Build, sign and dispatch one AWS JSON request, and read the answer back.
async fn dispatch(
    callback: &EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: &str,
) -> Result<Answer, Refusal> {
    sent(callback, api, region, operation, body)
        .await
        .map_err(|detail| match detail {
            Sent::TooLarge => Refusal::ResponseTooLarge,
            Sent::NotMade(detail) => Refusal::NotMade(detail),
        })
}

/// Why one dispatch produced no answer.
enum Sent {
    /// The answer is larger than [`MAX_RESPONSE_BYTES`].
    TooLarge,
    /// The request could not be built, signed or dispatched.
    NotMade(String),
}

/// The dispatch itself.
async fn sent(
    callback: &EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: &str,
) -> Result<Answer, Sent> {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{}/", api.host(region)))
        .header("host", api.host(region))
        .header("content-type", api.content_type())
        .header(TARGET_HEADER, format!("{}.{operation}", api.target_prefix))
        .body(body.as_bytes().to_vec())
        .map_err(|error| Sent::NotMade(error.to_string()))?;
    ::service_common::in_process_sigv4::sign_callback_request(
        &mut request,
        api.signing_name,
        region,
        callback,
    )
    .await
    .map_err(Sent::NotMade)?;
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| Sent::NotMade(error.to_string()))?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| Sent::NotMade(error.to_string()))?;
    let status = response.status().as_u16();
    let header = |name: &str| response.headers().get(name).map(|value| value.to_string());
    let error_type = header(ERROR_TYPE_HEADER);
    let request_id = header(REQUEST_ID_HEADER);
    let payload = response.body().bytes().ok_or_else(|| {
        Sent::NotMade("the in-process answer carried a streaming body".to_string())
    })?;
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err(Sent::TooLarge);
    }
    Ok(Answer {
        status,
        body: String::from_utf8_lossy(payload).into_owned(),
        error_type,
        request_id,
    })
}

/// The longest operation name a call may address.
///
/// Every AWS operation name is far shorter; the bound is here because the name travels in a
/// request HEADER, so its length is the host's to decide and not the guest's.
const MAX_OPERATION_NAME: usize = 128;

/// Whether `operation` is a name an AWS JSON target header can carry: a non-empty run of
/// ASCII letters and digits, no longer than [`MAX_OPERATION_NAME`].
///
/// This is what keeps a guest-supplied string out of the request's header structure: a name
/// carrying a separator, a control character or a newline is not an operation name, so no
/// request is built for it.
fn well_formed_operation(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= MAX_OPERATION_NAME
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        && operation.starts_with(|first: char| first.is_ascii_uppercase())
}

/// The longest Region name a call may name, for the same reason: it travels in the
/// request's host and in its credential scope.
const MAX_REGION_NAME: usize = 64;

/// Whether `region` is a Region name at all: a non-empty run of lower-case ASCII letters,
/// digits and hyphens, no longer than [`MAX_REGION_NAME`].
fn well_formed_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{reachable_services, well_formed_operation, well_formed_region, APIS};

    /// Every entry of the table names ONE service's wire facts, and the two tokens that
    /// name EventBridge reach the same service: an SDK client is built with the service's
    /// SDK id as often as with its endpoint prefix, and both are that service.
    #[test]
    fn the_table_names_each_reachable_service_once_per_token() {
        for api in APIS {
            assert!(
                !api.sdk_token.is_empty()
                    && !api.signing_name.is_empty()
                    && !api.endpoint_prefix.is_empty()
                    && !api.target_prefix.is_empty(),
                "case {:?}: every wire fact of a reachable service is stated",
                api.sdk_token
            );
            assert!(
                api.json_version == "1.0" || api.json_version == "1.1",
                "case {:?}: expected an AWS JSON protocol version, got {:?}",
                api.sdk_token,
                api.json_version
            );
        }
        let tokens = reachable_services();
        let mut sorted = tokens.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            tokens.len(),
            "case {tokens:?}: expected each service token once, got {sorted:?}"
        );
    }

    /// What a guest may name as an operation and as a Region, over a TABLE whose rows
    /// differ along the forms a header or a host must never be given: the rule is that only
    /// a plain name passes, so everything carrying a separator, a space, a control
    /// character or the wrong case is refused.
    #[test]
    fn only_a_plain_name_reaches_a_request_header_or_host() {
        let operations = [
            ("an operation name", "SendMessage", true),
            ("one carrying a digit", "DynamoDB20120810", true),
            ("the empty name", "", false),
            ("a lower-case name", "sendMessage", false),
            ("one carrying a dot", "AmazonSQS.SendMessage", false),
            ("one carrying a newline", "SendMessage\r\nHost: elsewhere", false),
            ("one carrying a space", "Send Message", false),
            ("one carrying a hyphen", "Send-Message", false),
            ("one past the length bound", &"A".repeat(129), false),
        ];
        for (case, operation, expected) in operations {
            let actual = well_formed_operation(operation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected well_formed_operation({operation:?}) to be \
                 {expected}, got {actual}"
            );
        }
        let regions = [
            ("a Region name", "us-west-2", true),
            ("another Region name", "ap-southeast-4", true),
            ("the empty name", "", false),
            ("one carrying an upper-case letter", "US-WEST-2", false),
            ("one carrying a dot", "us-west-2.evil", false),
            ("one carrying a slash", "us-west-2/path", false),
            ("one carrying a newline", "us-west-2\nX: y", false),
            ("one past the length bound", &"a".repeat(65), false),
        ];
        for (case, region, expected) in regions {
            let actual = well_formed_region(region);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected well_formed_region({region:?}) to be {expected}, \
                 got {actual}"
            );
        }
    }
}
