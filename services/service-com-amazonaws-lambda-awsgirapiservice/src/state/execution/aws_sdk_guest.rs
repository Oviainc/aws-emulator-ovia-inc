// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a Python handler imports: the `boto3` and `botocore` modules this runtime
//! supplies, in Python, over the one host function that makes a call
//! ([`super::aws_api`]).
//!
//! AWS Lambda's managed `python3.x` runtimes ship the AWS SDK for Python on the function's
//! import path, so `import boto3` at module scope succeeds and the handler can call another
//! service without shipping a dependency of its own. These modules are that import path's
//! contents here. They are the SDK's CLIENT interface — `boto3.client(...)`, an operation
//! per method, `botocore.exceptions.ClientError` for a refusal — because that is the
//! interface a handler calls a service through.
//!
//! What they do and do not do:
//!
//! * A method name is turned into the operation it names (`send_message` ->
//!   `SendMessage`) and its keyword arguments ARE the operation's input document, which is
//!   what an AWS JSON protocol puts on the wire; the host builds, signs and dispatches the
//!   request ([`super::aws_api`]) and the answer's document is what the method returns,
//!   with the `ResponseMetadata` the answer itself carried.
//! * A refusal is raised as `botocore.exceptions.ClientError`, carrying the service's own
//!   error code and message under the `response` member and rendering the sentence botocore
//!   renders, so a handler that catches one and reports `response["Error"]["Code"]` reads
//!   the emulated service's own answer.
//! * `client.exceptions.<ShapeName>` is the modelled error class for that name, a subclass
//!   of `ClientError`, so `except client.exceptions.QueueDoesNotExist` catches exactly that
//!   refusal and nothing else.
//! * There is no resource interface and no paginator: `boto3.resource(...)` and
//!   `client.get_paginator(...)` raise rather than pretending to serve one. A blob member
//!   travels as the base64 text an AWS JSON protocol carries it as, in both directions, and
//!   `bytes` a handler passes are encoded to it.
//! * Nothing here is the containment. The services that can be reached, the bounds on a
//!   call and the signature it carries are all host code's ([`super::aws_api`],
//!   [`super::python`]); a handler that rebinds or deletes a name in these modules changes
//!   none of them.

use super::package::PackageModule;

/// The modules this runtime puts on a Python handler's import path, as the SDK it ships.
///
/// Resolved AFTER the deployment package's own entries, so a package that ships its own
/// `boto3` wins — which is the order a real runtime resolves them in, where the package
/// directory precedes the runtime's own site-packages.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    [
        ("botocore/__init__.py", BOTOCORE_SOURCE),
        ("botocore/exceptions.py", BOTOCORE_EXCEPTIONS_SOURCE),
        ("botocore/client.py", BOTOCORE_CLIENT_SOURCE),
        ("boto3/__init__.py", BOTO3_SOURCE),
        ("boto3/session.py", BOTO3_SESSION_SOURCE),
    ]
    .into_iter()
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source: source.to_string(),
    })
    .collect()
}

/// `botocore`: the package the client interface and the exceptions live under.
const BOTOCORE_SOURCE: &str = r#"
"""The AWS SDK for Python's lower half, as this runtime supplies it."""
"#;

/// `botocore.exceptions`: the exception types a handler catches.
///
/// `ClientError`'s sentence is the one botocore renders — "An error occurred (<Code>) when
/// calling the <Operation> operation: <Message>" — because that sentence is what a handler
/// logs and what a caller reading the handler's report sees.
const BOTOCORE_EXCEPTIONS_SOURCE: &str = r#"
"""The exceptions an AWS SDK call raises."""


class BotoCoreError(Exception):
    """A failure of the SDK itself rather than an answer from a service."""

    fmt = "An unspecified error occurred"

    def __init__(self, **kwargs):
        Exception.__init__(self, self.fmt.format(**kwargs))
        self.kwargs = kwargs


class DataNotFoundError(BotoCoreError):
    fmt = "Unable to load data for: {data_path}"


class UnknownServiceError(DataNotFoundError):
    fmt = "Unknown service: '{service_name}'. Valid service names are: {known_service_names}"


class ParamValidationError(BotoCoreError):
    fmt = "Parameter validation failed:\n{report}"


class ReadTimeoutError(BotoCoreError):
    fmt = 'Read timeout on endpoint URL: "{endpoint_url}"'


class SdkCallNotMade(BotoCoreError):
    """The call could not be made at all, and why. Carries no answer from any service."""

    fmt = "{detail}"


class ClientError(Exception):
    """A service answered the call with a refusal."""

    MSG_TEMPLATE = (
        "An error occurred ({error_code}) when calling the {operation_name} "
        "operation: {error_message}"
    )

    def __init__(self, error_response, operation_name):
        error = error_response.get("Error", {})
        Exception.__init__(
            self,
            self.MSG_TEMPLATE.format(
                error_code=error.get("Code", "Unknown"),
                error_message=error.get("Message", "Unknown"),
                operation_name=operation_name,
            ),
        )
        self.response = error_response
        self.operation_name = operation_name
"#;

/// `botocore.client`: the client itself, and the one host call behind every method.
const BOTOCORE_CLIENT_SOURCE: &str = r##"
"""One AWS SDK client: a method per operation, over the runtime's own call seam."""

import base64
import json

from botocore.exceptions import (
    ClientError,
    ReadTimeoutError,
    SdkCallNotMade,
    UnknownServiceError,
)

_host = __import__("_lambda_host")


def _operation_name(method):
    """The operation a client method names: `send_message` -> `SendMessage`."""
    return "".join(part[:1].upper() + part[1:] for part in method.split("_") if part)


def _encoded(value):
    """A value `json` does not write by itself.

    An AWS JSON protocol carries a blob as base64 text, so `bytes` a handler passes are
    encoded to it. Anything else is a value this SDK cannot send, and says so.
    """
    if isinstance(value, (bytes, bytearray)):
        return base64.b64encode(bytes(value)).decode("ascii")
    raise TypeError("%r is not a value an AWS SDK call can carry" % (value,))


def _error_code(error_type, document):
    """The error code a refusal names, from the header or the document's own member."""
    for named in (error_type, document.get("__type"), document.get("code")):
        if not named:
            continue
        shape = named.split("#")[-1].split(":")[0].strip()
        if shape:
            return shape
    return "Unknown"


def _error_message(document):
    for member in ("message", "Message", "errorMessage"):
        text = document.get(member)
        if isinstance(text, str):
            return text
    return ""


class _ClientExceptions:
    """`client.exceptions`: the modelled error class for each error code."""

    def __init__(self, service_name):
        self._service_name = service_name
        self._classes = {"ClientError": ClientError}

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        return self.from_code(name)

    def from_code(self, code):
        held = self._classes.get(code)
        if held is None:
            held = type(str(code), (ClientError,), {})
            self._classes[code] = held
        return held


class _ClientMeta:
    """`client.meta`: what the client was built for."""

    def __init__(self, service_name, region_name, endpoint_url):
        self.service_model = _ServiceModel(service_name)
        self.region_name = region_name
        self.endpoint_url = endpoint_url


class _ServiceModel:
    def __init__(self, service_name):
        self.service_name = service_name
        self.service_id = service_name


class Client:
    """A client for one service in one Region."""

    def __init__(self, service_name, region_name=None, **_ignored):
        import os

        self._service_name = service_name
        self._region_name = (
            region_name
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or ""
        )
        self._endpoint_url = "https://%s.%s.amazonaws.com" % (
            service_name,
            self._region_name,
        )
        self.exceptions = _ClientExceptions(service_name)
        self.meta = _ClientMeta(service_name, self._region_name, self._endpoint_url)

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        operation_name = _operation_name(name)

        def method(**parameters):
            return self._call(operation_name, parameters)

        method.__name__ = name
        return method

    def can_paginate(self, operation_name):
        return False

    def get_paginator(self, operation_name):
        raise SdkCallNotMade(
            detail="the AWS SDK this runtime supplies has no paginator; call %s in a "
            "loop over its own continuation member instead" % (operation_name,)
        )

    def close(self):
        return None

    def _call(self, operation_name, parameters):
        answered = json.loads(
            _host.aws(
                self._service_name,
                operation_name,
                self._region_name,
                json.dumps(parameters, default=_encoded),
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            self._raise_refusal(refused, answered, operation_name)
        status = answered.get("status", 0)
        text = answered.get("body") or ""
        try:
            document = json.loads(text) if text else {}
        except ValueError:
            document = {}
        if not isinstance(document, dict):
            document = {}
        metadata = {
            "RequestId": answered.get("requestId"),
            "HTTPStatusCode": status,
            "RetryAttempts": 0,
        }
        if 200 <= status < 300:
            document["ResponseMetadata"] = metadata
            return document
        code = _error_code(answered.get("errorType"), document)
        response = {
            "Error": {"Code": code, "Message": _error_message(document)},
            "ResponseMetadata": metadata,
        }
        raise self.exceptions.from_code(code)(response, operation_name)

    def _raise_refusal(self, refused, answered, operation_name):
        detail = answered.get("detail") or ""
        if refused == "unknown_service":
            raise UnknownServiceError(
                service_name=self._service_name, known_service_names=detail
            )
        if refused == "unknown_operation":
            raise AttributeError(
                "'%s' object has no attribute '%s'"
                % (self._service_name, operation_name)
            )
        if refused == "timed_out":
            raise ReadTimeoutError(endpoint_url=self._endpoint_url)
        raise SdkCallNotMade(detail=detail)
"##;

/// `boto3`: the names a handler imports.
const BOTO3_SOURCE: &str = r#"
"""The AWS SDK for Python, as this runtime supplies it."""

from boto3.session import Session


def client(*args, **kwargs):
    """A client for one service, as `boto3.client(...)` builds one."""
    return Session().client(*args, **kwargs)


def resource(*args, **kwargs):
    from botocore.exceptions import SdkCallNotMade

    raise SdkCallNotMade(
        detail="the AWS SDK this runtime supplies has no resource interface; build a "
        "client with boto3.client(...) instead"
    )


def setup_default_session(**kwargs):
    return None
"#;

/// `boto3.session`: the session a client is built from.
const BOTO3_SESSION_SOURCE: &str = r#"
"""The session a client is built from."""

from botocore.client import Client
from botocore.exceptions import SdkCallNotMade


class Session:
    def __init__(self, region_name=None, **_ignored):
        self.region_name = region_name

    def client(self, service_name, region_name=None, **kwargs):
        return Client(service_name, region_name=region_name or self.region_name, **kwargs)

    def resource(self, *args, **kwargs):
        raise SdkCallNotMade(
            detail="the AWS SDK this runtime supplies has no resource interface; build a "
            "client with boto3.client(...) instead"
        )
"#;
