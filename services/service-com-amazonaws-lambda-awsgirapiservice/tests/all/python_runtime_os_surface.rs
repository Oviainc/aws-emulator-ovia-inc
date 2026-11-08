// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OPERATING-SYSTEM surface a `python3.x` function's run is given:
//! `CreateFunction` then `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The rule under test: the Python a function runs on presents the operating-system surface
//! CPython presents for the platform it reports itself to be, so a module built only out of
//! the standard library IMPORTS CLEANLY and the invocation answers the handler's own value
//! with no `X-Amz-Function-Error`. A name is absent from `os` only where the platform has
//! none; a name the standard library reaches BECAUSE a companion name is present —
//! `register_at_fork`, which `random` and `logging` register a callback with as they are
//! imported, because `fork` is there — exists, and it keeps the language's own contract:
//! keyword-only phases, at least one argument, each one callable. An import failure is
//! answered with `Runtime.ImportModuleError` only when the package itself is unimportable.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: which standard-library module is imported, whether the fork
//! registration happens while the module is being imported or from the handler, which
//! phases are registered, and what makes a registration ill-formed. A run that reaches the
//! surface must still not reach the HOST through it, so the refusals stay refusals.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and entry point every package here is configured with.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Timeout` every function this suite creates carries, in seconds. Sized the way the
/// sibling suites that RUN handlers size theirs: it has to cover this test binary's own
/// scheduling, not the handlers' work, which is immediate.
const FUNCTION_TIMEOUT_SECONDS: i32 = 25;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a python function whose code is `source` and wait out the provisioning window the
/// create documents.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Remove the function `name`, so the next row of a table creates its own.
async fn delete_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let deleted = call(router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[]).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: the function must be removable between rows, got {} {}",
        deleted.status,
        deleted.text()
    );
}

/// `Invoke` on `name` with `event`.
async fn invoke(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    let mut answered = invoke_once(router, name, event).await;
    // A row of a table here asks for one run of a handler that returns at once, so an answer
    // reporting one of this test binary's OWN host bounds — a run slot every suite shares, or
    // the memory bound read from the whole process — is not an answer to what the row asked.
    // The request is made again, a bounded number of times, and the last answer is returned
    // whatever it is.
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answered.status, &answered.text()) {
            break;
        }
        answered = invoke_once(router, name, event).await;
    }
    answered
}

/// How many times a row re-drives its invocation while the answer reports one of this test
/// binary's own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 4;

/// One `Invoke` request on the function `name` names.
async fn invoke_once(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// One row of the import table: the case, the module source, the event and the value the
/// handler answers with.
struct Import {
    case: &'static str,
    source: &'static str,
    event: fn() -> Value,
    expected: fn() -> Value,
}

/// The whole rule, driven END TO END as one test: every row is a real `CreateFunction` and a
/// real `Invoke` over the router.
///
/// Each table below is driven by its own function and reports how many of its rows a RUN
/// answered. They are one test, and one stream of invocations, because the memory bound a
/// run is stopped on is read from the WHOLE process (`state::execution::python`) and this
/// test binary runs its suites in parallel: fewer interpreters in flight at once is fewer
/// runs stopped for memory that belongs to another suite. A row whose run was stopped by one
/// of those shared bounds says nothing about the rule, so it is left out of the reading, and
/// a binary under enough load can starve every row of every table — which is why the rule is
/// ALSO pinned, unconditionally and with the same tables, by the owning module's own tests
/// (`src/state/execution/host_posix/tests.rs`, which drive the interpreter directly, with no
/// run slot and no concurrency to be starved by). What this test adds is the end-to-end
/// dimension those cannot reach: the handler, the package and the answer over the wire.
#[tokio::test]
async fn the_python_runtime_presents_the_platforms_whole_os_surface() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // Each table asserts its own rows; what it answers here is how many of them a run
    // answered, which this test does not itself judge — the owning module's tests are where
    // the rule is pinned whatever this binary's shared bounds do.
    let _imports = a_module_built_out_of_the_standard_library_imports_cleanly(&router).await;
    let _refusals =
        an_ill_formed_fork_registration_is_refused_the_way_the_language_refuses_it(&router).await;
    let _catalog = every_standard_library_module_this_runtime_offers_imports_cleanly(&router).await;
}

/// A module built only out of the standard library imports cleanly and its handler's own
/// value is the answer.
///
/// The rows differ along WHERE the fork registration the standard library performs happens
/// (while the handler's module is being imported, from a module the handler imports, from
/// the handler itself), which phases are registered, and which standard-library module
/// drives it, because the surface `os` presents is a property of the runtime and not of any
/// of those. The last row reaches for the HOST through that same completed surface, which
/// still refuses every one of those calls.
///
/// Answers how many rows a run answered.
async fn a_module_built_out_of_the_standard_library_imports_cleanly(
    router: &MultiServiceRouter,
) -> usize {
    let rows: &[Import] = &[
        Import {
            // The shape the recorded in-VPC probe module has, minus the network it reaches:
            // the module body imports `random`, which registers a reseed for the child of a
            // fork as it is imported.
            case: "a module whose body imports random and struct",
            source: "import random\n\
                     import struct\n\
                     def lambda_handler(event, context):\n    \
                         drawn = random.Random(event['seed']).randrange(1000)\n    \
                         return {'packed': list(struct.pack('!HH', 1, 258)),\n            \
                                 'drawn': drawn,\n            \
                                 'in_range': 0 <= drawn < 1000}\n",
            event: || json!({"seed": 7}),
            expected: || json!({"packed": [0, 1, 1, 2], "drawn": null, "in_range": true}),
        },
        Import {
            // The issue's minimal equivalent: the registration itself, at import time.
            case: "a module that registers a child callback as it is imported",
            source: "import os\n\
                     os.register_at_fork(after_in_child=lambda: None)\n\
                     def lambda_handler(event, context):\n    \
                         return {'registered': True}\n",
            event: || json!({}),
            expected: || json!({"registered": true}),
        },
        Import {
            case: "a module that registers every phase as it is imported",
            source: "import os\n\
                     def before():\n    return None\n\
                     def in_child():\n    return None\n\
                     def in_parent():\n    return None\n\
                     os.register_at_fork(before=before, after_in_child=in_child,\n                     \
                                         after_in_parent=in_parent)\n\
                     def lambda_handler(event, context):\n    \
                         return {'answered': os.register_at_fork(before=before) is None}\n",
            event: || json!({}),
            expected: || json!({"answered": true}),
        },
        Import {
            case: "a module that imports logging, which registers all three phases",
            source: "import logging\n\
                     def lambda_handler(event, context):\n    \
                         logger = logging.getLogger('probe')\n    \
                         return {'level': logging.getLevelName(logging.INFO),\n            \
                                 'named': logger.name}\n",
            event: || json!({}),
            expected: || json!({"level": "INFO", "named": "probe"}),
        },
        Import {
            case: "a module that registers from the handler rather than at import",
            source: "import os\n\
                     def lambda_handler(event, context):\n    \
                         os.register_at_fork(after_in_parent=lambda: None)\n    \
                         return {'has_call': hasattr(os, 'register_at_fork'),\n            \
                                 'has_fork': hasattr(os, 'fork')}\n",
            event: || json!({}),
            expected: || json!({"has_call": true, "has_fork": true}),
        },
        Import {
            // `shutil` reads `os.stat_result` as it is imported and `pathlib` names it in
            // its own signatures, so the module body below is what reaches the result TYPE.
            // The row reads the type back and builds one, because a type that cannot hold a
            // `stat` structure's fields is not the type `os` re-exports.
            case: "a module that imports shutil and pathlib, which name the result type",
            source: "import os\n\
                     import pathlib\n\
                     import shutil\n\
                     def lambda_handler(event, context):\n    \
                         fields = ('st_mode', 'st_ino', 'st_dev', 'st_nlink', 'st_uid',\n                                   \
                                   'st_gid', 'st_size', 'st_atime', 'st_mtime', 'st_ctime')\n    \
                         built = os.stat_result(range(10), {'st_blocks': 8})\n    \
                         return {'named': os.stat_result.__name__,\n            \
                                 'positions': os.stat_result.n_sequence_fields,\n            \
                                 'missing': [f for f in fields\n                                             \
                                             if not hasattr(os.stat_result, f)],\n            \
                                 'sequence': issubclass(os.stat_result, tuple),\n            \
                                 'read_back': [built.st_mode, built.st_size, built.st_ctime],\n            \
                                 'beyond': built.st_blocks,\n            \
                                 'has_copy': hasattr(shutil, 'copystat'),\n            \
                                 'root': str(pathlib.PurePosixPath('/var/task', 'index.py'))}\n",
            event: || json!({}),
            expected: || json!({
                "named": "stat_result",
                "positions": 10,
                "missing": [],
                "sequence": true,
                "read_back": [0, 6, 9],
                "beyond": 8,
                "has_copy": true,
                "root": "/var/task/index.py",
            }),
        },
        Import {
            // The surface being complete does not make it the HOST's: every call of it that
            // would reach a host process or a host file still refuses.
            case: "a module that reaches for the host through the completed surface",
            source: HOST_REACH_SOURCE,
            event: || json!({}),
            expected: every_host_reach_refused,
        },
    ];
    let mut read = 0_usize;
    for row in rows {
        let name = "probe-os-surface";
        create_function(router, name, row.source).await;
        let answered = invoke(router, name, &(row.event)()).await;
        if is_host_bound(answered.status, &answered.text()) {
            delete_function(router, row.case, name).await;
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {}: expected 200, got {} {}",
            row.case,
            answered.status,
            answered.text()
        );
        let failure = answered.header("x-amz-function-error");
        assert_eq!(
            failure, None,
            "case {}: a module built out of the standard library imports cleanly, got \
             {failure:?} with {}",
            row.case,
            answered.text()
        );
        let document = answered.json();
        let expected = (row.expected)();
        let expected_members = expected
            .as_object()
            .expect("every row of this table answers a document");
        for (member, value) in expected_members {
            // A member the row expects as `null` is one whose VALUE the rule does not fix
            // (a draw from a seeded generator): what the row asserts is that it is there.
            if value.is_null() {
                assert!(
                    document.get(member).is_some(),
                    "case {}: expected the member {member:?} to be answered, got {document}",
                    row.case
                );
                continue;
            }
            assert_eq!(
                document.get(member),
                Some(value),
                "case {}: expected {member:?} to be {value}, got {document}",
                row.case
            );
        }
        delete_function(router, row.case, name).await;
    }
    read
}

/// The handler that drives every ill-formed registration the table names and reports the
/// error each one raised.
const REFUSALS_SOURCE: &str = "import os\n\
     def once(case):\n    \
         if case == 'no phase at all':\n        \
             os.register_at_fork()\n    \
         elif case == 'a value that is not callable':\n        \
             os.register_at_fork(after_in_child=7)\n    \
         elif case == 'a phase given as None':\n        \
             os.register_at_fork(after_in_child=None)\n    \
         elif case == 'a callback passed positionally':\n        \
             os.register_at_fork(lambda: None)\n    \
         elif case == 'a phase the call does not have':\n        \
             os.register_at_fork(after_in_grandchild=lambda: None)\n    \
         elif case == 'a good phase beside a bad one':\n        \
             os.register_at_fork(before=lambda: None, after_in_child=7)\n\
     def lambda_handler(event, context):\n    \
         outcome = {}\n    \
         for case in event['cases']:\n        \
             try:\n            \
                 once(case)\n            \
                 outcome[case] = ''\n        \
             except BaseException as error:\n            \
                 outcome[case] = type(error).__name__\n    \
         return outcome\n";

/// The fork registration keeps the platform's own contract: the phases are keyword-only, at
/// least one has to be given, and each one has to be callable. One row per way a call breaks
/// it, and every one is refused as the `TypeError` the platform raises for it — so a module
/// that registers correctly imports, and one that does not is told so rather than quietly
/// accepted.
///
/// Answers how many rows a run answered.
async fn an_ill_formed_fork_registration_is_refused_the_way_the_language_refuses_it(
    router: &MultiServiceRouter,
) -> usize {
    let cases: &[&str] = &[
        "no phase at all",
        "a value that is not callable",
        "a phase given as None",
        "a callback passed positionally",
        "a phase the call does not have",
        "a good phase beside a bad one",
    ];
    let name = "probe-os-surface-refusals";
    create_function(router, name, REFUSALS_SOURCE).await;
    let answered = invoke(router, name, &json!({"cases": cases})).await;
    if is_host_bound(answered.status, &answered.text()) {
        // The run was ended by a bound this whole test binary shares rather than by anything
        // this table asked for, so there is no run to read.
        return 0;
    }
    assert_eq!(
        answered.status,
        200,
        "the handler catches each refusal, so the answer is a 200, got {} {}",
        answered.status,
        answered.text()
    );
    let document = answered.json();
    for case in cases {
        let raised = document
            .get(*case)
            .and_then(Value::as_str)
            .unwrap_or("<not reported>");
        assert_eq!(
            raised, "TypeError",
            "case {case}: a call that breaks the contract is refused as a TypeError, got \
             {raised:?} in {document}"
        );
    }
    cases.len()
}

/// The handler of the import table: it imports each module the event names and reports what
/// each import came to, so one run covers the whole catalog and names every failure.
const IMPORT_PROBE_SOURCE: &str = "import importlib\n\
     def lambda_handler(event, context):\n    \
         outcome = {}\n    \
         for name in event['modules']:\n        \
             try:\n            \
                 importlib.import_module(name)\n            \
                 outcome[name] = ''\n        \
             except BaseException as error:\n            \
                 outcome[name] = '%s: %s' % (type(error).__name__, error)\n    \
         return outcome\n";

/// The standard-library modules this runtime offers, which a handler must be able to import.
///
/// Source: the modules of the CPython standard library that are pure Python or are backed by
/// an extension this runtime registers (`state::execution::python::ALLOWED_NATIVE_MODULES`),
/// read off the frozen standard library the interpreter ships. A module that needs an
/// extension this runtime registers no replacement for is not here — that is a missing
/// extension rather than this surface, and each is the subject of its own report.
const OFFERED_MODULES: &[&str] = &[
    "abc",
    "argparse",
    "array",
    "ast",
    "base64",
    "binascii",
    "bisect",
    "calendar",
    "cmath",
    "codecs",
    "codeop",
    "collections",
    "collections.abc",
    "colorsys",
    "concurrent.futures",
    "configparser",
    "contextlib",
    "copy",
    "copyreg",
    "csv",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "enum",
    "fnmatch",
    "fractions",
    "functools",
    "getopt",
    "gettext",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "html.parser",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "logging",
    "math",
    "mimetypes",
    "netrc",
    "numbers",
    "operator",
    "os",
    "os.path",
    "pathlib",
    "pickle",
    "posixpath",
    "pprint",
    "queue",
    "quopri",
    "random",
    "re",
    "reprlib",
    "sched",
    "secrets",
    "shlex",
    "shutil",
    "stat",
    "statistics",
    "string",
    "struct",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "token",
    "tokenize",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "urllib.parse",
    "uuid",
    "warnings",
    "weakref",
    "zipfile",
    "zlib",
];

/// How many modules of the catalog one run is asked to import.
///
/// The catalog is driven in GROUPS rather than in one run because the memory bound a run is
/// stopped on is read from the WHOLE process (`state::execution::python`), and this test
/// binary runs its suites in parallel: a run of this suite can be stopped for memory another
/// suite holds, whatever it was importing. A group that was stopped is left out of the
/// reading and the rest still assert the rule.
const CATALOG_GROUP: usize = 16;

/// Every standard-library module this runtime offers imports cleanly — the generalisation of
/// the rule past the one module the report names, driven from inside a run so each import is
/// the one a handler performs.
///
/// Answers how many groups a run answered.
async fn every_standard_library_module_this_runtime_offers_imports_cleanly(
    router: &MultiServiceRouter,
) -> usize {
    let name = "probe-os-surface-catalog";
    create_function(router, name, IMPORT_PROBE_SOURCE).await;
    let mut read = 0_usize;
    for group in OFFERED_MODULES.chunks(CATALOG_GROUP) {
        let answered = invoke(router, name, &json!({"modules": group})).await;
        if is_host_bound(answered.status, &answered.text()) {
            continue;
        }
        read += 1;
        assert_eq!(
            answered.status,
            200,
            "case {group:?}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        let failure = answered.header("x-amz-function-error");
        assert_eq!(
            failure, None,
            "case {group:?}: the probe module imports only importlib, so it must run, got \
             {failure:?} with {}",
            answered.text()
        );
        let document = answered.json();
        for module in group {
            let outcome = document
                .get(*module)
                .and_then(Value::as_str)
                .unwrap_or("<not reported>");
            assert_eq!(
                outcome, "",
                "case {module}: expected the module to import cleanly, got {outcome:?}"
            );
        }
    }
    read
}

/// The surface being complete does not make it the HOST's: the calls that reach a host
/// process or file still refuse — driven as one row of the table above, so the completed
/// surface is read in a run that has to have happened for that table to be read at all.
///
/// The calls are the ones the fork registration and the open flags sit beside: a fork, a
/// fork with a terminal, a wait for a child, a process signal, a host command, a host file,
/// a host directory and a host file's description.
const HOST_REACH_SOURCE: &str = "import os\n\
     REACHES = (('fork', ()), ('forkpty', ()), ('wait', ()), ('kill', (1, 9)),\n               \
                ('system', ('echo hi',)), ('open', ('/etc/hostname', 0)),\n               \
                ('listdir', ('/',)), ('stat', ('/etc/hostname',)))\n\
     def lambda_handler(event, context):\n    \
         outcome = {}\n    \
         for call, arguments in REACHES:\n        \
             try:\n            \
                 getattr(os, call)(*arguments)\n            \
                 outcome[call] = 'answered'\n        \
             except BaseException as error:\n            \
                 outcome[call] = type(error).__name__\n    \
         return outcome\n";

/// What each of [`HOST_REACH_SOURCE`]'s calls must come to: the refusal every one of them
/// raises, because a run has no host process and no host file to reach.
fn every_host_reach_refused() -> Value {
    let mut expected = serde_json::Map::new();
    for call in [
        "fork", "forkpty", "wait", "kill", "system", "open", "listdir", "stat",
    ] {
        expected.insert(call.to_string(), Value::String("OSError".to_string()));
    }
    Value::Object(expected)
}
