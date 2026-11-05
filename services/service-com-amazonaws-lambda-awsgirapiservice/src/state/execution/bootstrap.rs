// SPDX-License-Identifier: CC0-1.0

//! The RUNTIME the handler runs under, in the language the handler is written in.
//!
//! A Python runtime's job is small and entirely defined by what the handler sees: import
//! the handler's module out of the deployment package, build the context object, call the
//! handler with the event and the context, and report back either what it returned or the
//! error document Lambda answers for an exception. That is this source: it runs INSIDE the
//! interpreter [`super::python`] builds, because every one of those steps is a Python one —
//! `importlib` resolves the module, `json` reads the event and writes the response, and
//! `traceback` renders the stack of an exception the handler raised.
//!
//! What it is NOT is the containment. Nothing here is a security boundary: a module the
//! guest must not reach is not registered by the host at all ([`super::python`]), so this
//! source cannot be made to hand one over by deleting or rebinding a name in it. The two
//! host functions it does use are the ones the host deliberately exposes — the log the
//! handler's output goes into, and the bounded sleep — and both are enforced in host code.
//!
//! The protocol between host and runtime is two globals, so the host's own interpreter
//! surface stays as small as a string in and a string out:
//!
//! * `__lambda_input` — a JSON document the host sets before this source runs: the
//!   handler name, the package's modules, the event, the environment the invocation runs
//!   with, and the deadline.
//! * `__lambda_entry` — the one function this source leaves behind, which the host calls
//!   once the bounds are in place; it answers a JSON document carrying what the handler
//!   returned, or the error it raised.
//!
//! Nothing here imports an operating-system module: the environment the handler sees is
//! installed by the HOST into the module `os` reads it from, and the context's members are
//! read out of the document above rather than out of a module this source would otherwise
//! have to reach for.

/// The Python source of the runtime, evaluated once per invocation in a fresh interpreter.
pub(super) const RUNTIME_SOURCE: &str = r##"
import sys

# Everything this runtime itself needs, imported once, before the handler's own work
# begins and before the bounds that measure that work are in place.
import importlib
import importlib.util
import json
import time
import traceback

_host = __import__("_lambda_host")


class _LogStream:
    """One of the handler's output streams, writing into the invocation's log.

    The interpreter is built without the host's own standard streams, so this is the
    only `sys.stdout`/`sys.stderr` a handler has: `print()` and `logging` reach the
    invocation's log through it, and nothing reaches this process's stdio.
    """

    encoding = "utf-8"
    errors = "backslashreplace"
    name = "<log>"
    mode = "w"
    closed = False

    def write(self, text):
        if not isinstance(text, str):
            text = str(text)
        _host.log(text)
        return len(text)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        return None

    def isatty(self):
        return False

    def readable(self):
        return False

    def writable(self):
        return True

    def seekable(self):
        return False

    def fileno(self):
        raise OSError("the invocation log has no file descriptor")

    def close(self):
        return None


sys.stdout = _LogStream()
sys.stderr = _LogStream()
sys.stdin = None


class _PackageLoader:
    """Imports one module from the deployment package held in memory."""

    def __init__(self, path, sources):
        self._path = path
        self._sources = sources

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        source = self._sources[self._path]
        filename = "/var/task/" + self._path
        module.__dict__["__file__"] = filename
        code = compile(source, filename, "exec")
        exec(code, module.__dict__)

    def get_source(self, name):
        return self._sources[self._path]

    def is_package(self, name):
        return self._path.endswith("/__init__.py")


class _PackageFinder:
    """Resolves a module name against the deployment package's own entries.

    Installed ahead of every other finder, so the handler's own modules win over a
    standard-library module of the same name, exactly as the package directory does on
    the real runtime's `sys.path`.

    `spec_from_loader` is handed in rather than imported here: this runs INSIDE the
    import machinery, so an import of its own would re-enter the machinery.
    """

    def __init__(self, sources, spec_from_loader):
        self._sources = sources
        self._spec_from_loader = spec_from_loader

    def find_spec(self, name, path=None, target=None):
        key = name.replace(".", "/")
        for candidate, is_package in ((key + "/__init__.py", True), (key + ".py", False)):
            if candidate in self._sources:
                loader = _PackageLoader(candidate, self._sources)
                return self._spec_from_loader(name, loader, is_package=is_package)
        return None


class LambdaContext:
    """The context object the handler is called with.

    Every member is read from the ENVIRONMENT of the invocation, which is where the
    runtime interface reads them from: the function's name, version, memory size and log
    destination are environment variables of the invocation. The host hands that
    environment in, so this runtime needs no operating-system module of its own.
    """

    def __init__(self, request, environment):
        self.aws_request_id = request["requestId"]
        self.invoked_function_arn = request["invokedFunctionArn"]
        self.log_group_name = environment.get("AWS_LAMBDA_LOG_GROUP_NAME")
        self.log_stream_name = environment.get("AWS_LAMBDA_LOG_STREAM_NAME")
        self.function_name = environment.get("AWS_LAMBDA_FUNCTION_NAME")
        self.function_version = environment.get("AWS_LAMBDA_FUNCTION_VERSION")
        self.memory_limit_in_mb = environment.get("AWS_LAMBDA_FUNCTION_MEMORY_SIZE")
        self.identity = None
        self.client_context = request["clientContext"]
        self.tenant_id = request["tenantId"]
        self._deadline_epoch_ms = request["deadlineEpochMs"]

    def get_remaining_time_in_millis(self):
        return max(0, int(self._deadline_epoch_ms - time.time() * 1000.0))

    def log(self, message):
        sys.stdout.write(message)


def _raised(error_type, error_message, stack_trace):
    return {
        "outcome": "raised",
        "errorType": error_type,
        "errorMessage": error_message,
        "stackTrace": stack_trace,
    }


def _bounded_sleep(seconds):
    return _host.sleep(float(seconds))


def _run(request):
    sources = request["modules"]
    sys.meta_path.insert(0, _PackageFinder(sources, importlib.util.spec_from_loader))
    time.sleep = _bounded_sleep

    handler = request["handler"]
    module_name, separator, attribute = handler.rpartition(".")
    if not separator or not module_name or not attribute:
        return _raised(
            "Runtime.MalformedHandlerName",
            "Bad handler '%s': not enough values to unpack (expected 2, got 1)" % handler,
            [],
        )

    try:
        module = importlib.import_module(module_name)
    except BaseException as error:
        return _raised(
            "Runtime.ImportModuleError",
            "Unable to import module '%s': %s" % (module_name, error),
            [],
        )

    try:
        entry = getattr(module, attribute)
    except AttributeError:
        return _raised(
            "Runtime.HandlerNotFound",
            "Handler '%s' missing on module '%s'" % (attribute, module_name),
            [],
        )

    event_text = request["event"]
    try:
        event = json.loads(event_text) if event_text else {}
    except ValueError as error:
        return _raised("Runtime.InvalidEvent", str(error), [])

    context = LambdaContext(request, request["environment"])

    try:
        returned = entry(event, context)
    except BaseException as error:
        trace = error.__traceback__
        # The frame this runtime called the handler from is not part of the handler's
        # own stack, so the trace starts at the handler.
        if trace is not None:
            trace = trace.tb_next
        return _raised(
            type(error).__name__,
            str(error),
            traceback.format_tb(trace) if trace is not None else [],
        )

    try:
        payload = json.dumps(returned)
    except BaseException as error:
        return _raised(
            "Runtime.MarshalError",
            "Unable to marshal response: %s" % error,
            [],
        )
    return {"outcome": "returned", "payload": payload}


def _main(payload):
    try:
        request = json.loads(payload)
    except BaseException as error:
        return json.dumps(_raised("Runtime.InternalError", str(error), []))
    try:
        return json.dumps(_run(request))
    except BaseException as error:
        return json.dumps(
            _raised(
                type(error).__name__,
                str(error),
                traceback.format_tb(error.__traceback__),
            )
        )


# The host calls this once the bounds are in place. Nothing above has run any of the
# caller's own code: the imports and definitions above are this runtime's own setup.
__lambda_entry = _main
"##;
