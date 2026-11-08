// SPDX-License-Identifier: CC0-1.0

//! The X-RAY DAEMON a traced invocation reports its own work to, and the DATAGRAM half of the
//! `socket` module a handler reaches it through.
//!
//! An instrumented Lambda handler does not call X-Ray: it writes its segment and subsegment
//! documents as UDP datagrams to the daemon the execution environment names in
//! `AWS_XRAY_DAEMON_ADDRESS`, and the daemon submits them. That is what this module is, in
//! two halves:
//!
//! * [`DATAGRAM_SOURCE`], the datagram sockets of the `socket` module this runtime supplies
//!   ([`super::guest_http::runtime_stdlib_modules`], which this source is appended to). The
//!   interpreter a run happens in registers no host socket extension at all — nothing of a run
//!   may open a connection ([`super::python`]) — so a socket here opens nothing: the ONE
//!   reachable destination is the daemon address this invocation's environment names, and every
//!   other destination raises the error a host with no route to it answers, exactly as the rest
//!   of that module's calls do.
//! * the DAEMON itself, in that same source: a datagram is read as the daemon's own framing —
//!   a header line declaring the format, then one segment document per line — and the documents
//!   it carries are submitted to X-Ray through `PutTraceSegments`, over the one AWS SDK seam a
//!   run makes a call on ([`super::aws_api`]), signed as the session of the function's own
//!   execution role. So a document a handler sends really is stored, under the trace the
//!   document itself names, and a function whose role is not granted `xray:PutTraceSegments`
//!   has its submission refused exactly as any other call of it would be.
//!
//! A datagram the framing does not admit is DROPPED rather than submitted, which is what the
//! daemon does with one: UDP reports nothing back, so a malformed datagram is lost and the
//! next well-formed one on the same socket still lands.
//!
//! Nothing here is the containment. Which services a run can reach, how large a request may
//! be and how many calls it may make are all host code's ([`super::aws_api`],
//! [`super::python`]); a handler that rebinds a name in this module changes none of them, and
//! the only destination it can deliver to is the in-process daemon either way.

/// The address the execution environment names the daemon at: "`AWS_XRAY_DAEMON_ADDRESS` –
/// The X-Ray daemon address in the format `IP_ADDRESS:PORT`" (the Lambda Developer Guide's
/// runtime environment variables), where the port is the `2000/udp` the X-Ray daemon
/// listens on.
///
/// The daemon is served IN PROCESS, by the source below, so the address a handler is given is
/// one its datagrams really reach rather than one it has to resolve.
pub const DAEMON_ADDRESS: &str = "127.0.0.1:2000";

/// The variable that address is reported in.
pub const DAEMON_ADDRESS_VARIABLE: &str = "AWS_XRAY_DAEMON_ADDRESS";

/// What a traced run does when a tracer finds no trace context: "`AWS_XRAY_CONTEXT_MISSING`
/// – For X-Ray tracing, Lambda sets this to `LOG_ERROR`" (the same guide), which has a
/// tracer log the miss and carry on rather than raise.
pub const CONTEXT_MISSING_VARIABLE: &str = "AWS_XRAY_CONTEXT_MISSING";

/// The value that variable carries.
pub const CONTEXT_MISSING_LOG_ERROR: &str = "LOG_ERROR";

/// The DATAGRAM half of the `socket` module this runtime supplies, appended to the names,
/// errors and conversions that module already states
/// ([`super::guest_http::runtime_stdlib_modules`]) — so there is ONE `socket` a handler can
/// import, and the only thing this adds to it is the daemon a datagram can reach.
pub const DATAGRAM_SOURCE: &str = r#"

# ---------------------------------------------------------------------------
# The datagram sockets of this runtime, and the X-Ray daemon behind them.
#
# A datagram addressed to the daemon address the environment names is delivered to the
# daemon, which submits the segment documents it carries to X-Ray; every other destination
# has no route, so addressing one raises as it does on a host with none.

import json as _json
import os as _os

_host = __import__("_lambda_host")

# The daemon's own framing: a datagram is a header line declaring the format and the
# version, then one segment document per line.
_DAEMON_FORMAT = "json"

# ENETUNREACH, as a host with no route to the address answers.
_ENETUNREACH = 101
_NO_ROUTE = (
    "a Lambda function in this emulator opens no connection: the only address its "
    "datagrams reach is the X-Ray daemon named by AWS_XRAY_DAEMON_ADDRESS"
)


def _daemon_address():
    """The address this invocation's environment names the daemon at."""
    return _os.environ.get("AWS_XRAY_DAEMON_ADDRESS") or ""


def _daemon_region():
    """The Region the submission is made in: the one the invocation runs in."""
    return _os.environ.get("AWS_REGION") or _os.environ.get("AWS_DEFAULT_REGION") or ""


def _named(address):
    """`address` as the `host:port` text the daemon address is written in."""
    if isinstance(address, (tuple, list)) and len(address) >= 2:
        return "%s:%s" % (address[0], address[1])
    if isinstance(address, (bytes, bytearray)):
        return bytes(address).decode("utf-8", "replace")
    return str(address)


def _datagram_text(data):
    if isinstance(data, (bytes, bytearray, memoryview)):
        return bytes(data).decode("utf-8", "replace")
    return str(data)


def _segment_documents(data):
    """The segment documents one datagram carries, or none when the framing is not the
    daemon's: a header line that is a JSON object declaring the format, and then one document
    per line. A datagram the framing does not admit is dropped whole, and a line that is not a
    JSON document is dropped on its own."""
    lines = [line for line in _datagram_text(data).split("\n") if line.strip()]
    if len(lines) < 2:
        return []
    try:
        header = _json.loads(lines[0])
    except ValueError:
        return []
    if not isinstance(header, dict) or header.get("format") != _DAEMON_FORMAT:
        return []
    documents = []
    for line in lines[1:]:
        try:
            parsed = _json.loads(line)
        except ValueError:
            continue
        if isinstance(parsed, dict):
            documents.append(line)
    return documents


def _delivered(data):
    """Submit what this datagram carries to X-Ray. Nothing is reported back: a datagram
    carries no answer, so a submission the service refused is lost exactly as a datagram the
    daemon could not read is."""
    documents = _segment_documents(data)
    if not documents:
        return
    try:
        _host.aws(
            "xray",
            "PutTraceSegments",
            _daemon_region(),
            _json.dumps({"TraceSegmentDocuments": documents}),
        )
    except Exception:
        return


class socket(object):
    """One socket of the invocation. A datagram socket delivers to the daemon; a stream
    socket has nothing to connect to."""

    def __init__(self, family=AF_INET, type=SOCK_STREAM, proto=0, fileno=None):
        self.family = family
        self.type = type
        self.proto = proto
        self._closed = False
        self._peer = None
        self._timeout = None

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        self.close()
        return False

    def __repr__(self):
        return "<socket family=%s type=%s proto=%s>" % (
            self.family,
            self.type,
            self.proto,
        )

    def close(self):
        self._closed = True

    def detach(self):
        self._closed = True
        return -1

    def fileno(self):
        return -1

    def settimeout(self, value):
        self._timeout = value

    def gettimeout(self):
        return self._timeout

    def setblocking(self, blocking):
        self._timeout = None if blocking else 0.0

    def getblocking(self):
        return self._timeout != 0.0

    def setsockopt(self, *arguments):
        return None

    def getsockopt(self, level, option, *arguments):
        return 0

    def bind(self, address):
        return None

    def getsockname(self):
        return ("0.0.0.0", 0)

    def getpeername(self):
        if self._peer is None:
            raise error(_ENETUNREACH, _NO_ROUTE)
        return self._peer

    def sendto(self, data, *arguments):
        if not arguments:
            raise TypeError("sendto() takes an address")
        self._deliver(data, _named(arguments[-1]))
        return len(_datagram_text(data).encode("utf-8"))

    def connect(self, address):
        self._reachable(_named(address))
        self._peer = address

    def connect_ex(self, address):
        try:
            self.connect(address)
        except OSError:
            return _ENETUNREACH
        return 0

    def send(self, data, flags=0):
        if self._peer is None:
            raise error(_ENETUNREACH, _NO_ROUTE)
        self._deliver(data, _named(self._peer))
        return len(_datagram_text(data).encode("utf-8"))

    def sendall(self, data, flags=0):
        self.send(data, flags)
        return None

    def recv(self, size, flags=0):
        raise error(_ENETUNREACH, _NO_ROUTE)

    def recvfrom(self, size, flags=0):
        raise error(_ENETUNREACH, _NO_ROUTE)

    def listen(self, backlog=0):
        raise error(_ENETUNREACH, _NO_ROUTE)

    def accept(self):
        raise error(_ENETUNREACH, _NO_ROUTE)

    def makefile(self, *arguments, **keywords):
        raise error(_ENETUNREACH, _NO_ROUTE)

    def shutdown(self, how):
        return None

    def _reachable(self, named):
        if self._closed:
            raise error("the socket is closed")
        daemon = _daemon_address()
        if self.type != SOCK_DGRAM or not daemon or named != daemon:
            raise error(_ENETUNREACH, _NO_ROUTE)

    def _deliver(self, data, named):
        self._reachable(named)
        _delivered(data)


SocketType = socket
"#;
