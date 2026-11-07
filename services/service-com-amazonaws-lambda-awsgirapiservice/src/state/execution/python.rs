// SPDX-License-Identifier: CC0-1.0

//! The PYTHON interpreter one invocation runs in, and everything that bounds and contains
//! it.
//!
//! A `python3.x` function's answer is whatever its handler returned, so the handler is
//! RUN: the deployment package's modules are compiled and executed by the interpreter this
//! module builds, in this process, and the value the handler returned is serialized by that
//! same interpreter. Nothing about a run is synthesised — a payload, a log line, an error
//! type or a duration here is the one the guest produced.
//!
//! # Containment
//!
//! The guest reaches exactly what this module registers, and nothing else:
//!
//! * the interpreter is built with the engine crate's host-reaching surface compiled OUT
//!   (`default-features = false`, see this crate's `Cargo.toml`), so no `ctypes` host
//!   loader, no host standard streams and no interpreter thread exist in the build;
//! * the native modules are an ALLOW-LIST ([`ALLOWED_NATIVE_MODULES`]) filtered out of the
//!   engine's own pool, so a module that reaches a socket, a subprocess, a memory map or a
//!   host library is never registered — it is therefore unreachable through `import`,
//!   `sys.modules`, `importlib` or `__import__`, however the guest rebinds or deletes
//!   names;
//! * the modules a Python program expects of its operating system are this module's OWN
//!   ([`host_posix_module`], [`host_thread_module`]): the environment is the invocation's
//!   own variables held in memory, the working directory is the task root the runtime
//!   reports, randomness comes from the host's own generator, and every call that would
//!   reach a host file, process or descriptor raises;
//! * the only host functions the guest can call are the four on [`HOST_MODULE`]: the
//!   invocation log, a sleep clamped to what is left of the run, the step hook below, and
//!   ONE AWS SDK call into this same emulator ([`super::aws_api`], which decides which
//!   services are reachable, signs the request itself and bounds it by what the run has
//!   left). No host path, host environment variable, host credential or other function's
//!   state is reachable from guest code.
//!
//! # Bounds
//!
//! Every bound lives in host code the guest cannot switch off:
//!
//! * STEPS and the DEADLINE are checked by [`Bounds::step`], called from the interpreter's
//!   trace hook, which the host installs and then REMOVES `sys.settrace` /
//!   `sys.setprofile` / `sys.gettrace` for, so no guest call can clear or replace it. The
//!   stop is LATCHED: once the budget is spent the run's answer is that stop, whatever the
//!   guest goes on to return or catch.
//! * MEMORY is sampled from outside the guest — this thread's faulted pages and the
//!   process's resident size — because the interpreter exposes no allocation seam; a
//!   sample past the run's ceiling ends the run.
//! * RECURSION is clamped on the interpreter before any guest code runs, and
//!   `sys.setrecursionlimit` is removed so the guest cannot raise it.
//! * The LOG and the RESPONSE are each capped, so a small request cannot make an
//!   unbounded answer.
//! * A long NATIVE call emits no trace event, so the deadline cannot land inside one.
//!   That is what the caller's own host watch and abandoned-run accounting
//!   ([`super::run_invocation`]) exist for: the CALLER is always answered inside its
//!   ceiling, and a run still going after its budget was spent is given up on.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rustpython_vm::function::FuncArgs;
use rustpython_vm::builtins::PyModule;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyRef, PyResult, Settings, VirtualMachine};

// What one run may spend and what it came to are the same for every engine, so they are
// the shared ones ([`super::outcome`]) rather than this interpreter's own.
pub use super::outcome::{Budget, Completion, Finished, Stop};

use super::aws_api::{AwsAccess, Refusal, MAX_CALLS_PER_RUN};
use super::outcome::{captured, Capture};
use super::package::PackageModule;

/// The name of the module the three host functions live on, as the runtime source imports
/// it.
const HOST_MODULE: &str = "_lambda_host";

/// Every native module this interpreter registers, out of the engine's own two pools.
///
/// This list IS the containment of the native surface: a name absent from it is dropped
/// from the interpreter's module table before any guest code runs, so no guest-side import
/// of it can succeed — not through `import`, not through `importlib`, not through
/// `__import__`, and not by dropping the module from `sys.modules` and importing it again
/// ([`prepare`] takes the builtin-module creation calls away too).
///
/// What is here is the interpreter's own machinery (the import system, the exception and
/// object model support, the bytecode and codec support) and the arithmetic, text,
/// hashing and data-structure modules the frozen standard library needs for a handler to
/// `import json`, `datetime`, `decimal`, `hashlib`, `base64`, `random`, `re` or `uuid` —
/// none of which reaches outside this process.
///
/// Deliberately absent, so that each stays unregistered: `_socket`, `_ssl`, `select`,
/// `_posixsubprocess`, `mmap`, `_ctypes`, `fcntl`, `termios`, `resource`, `pwd`, `grp`,
/// `syslog`, `_signal`, `_interpreters`, `_interpchannels`, `_interpqueues`,
/// `_testinternalcapi`, and every other module whose calls land on the host. `posix` and
/// `_thread` are absent for the same reason, and this module supplies its OWN of each
/// instead (see [`host_posix_module`], [`host_thread_module`]).
///
/// Two of the names here are registered but PATCHED before guest code runs, because the
/// frozen standard library needs the rest of each: `_io`, whose host file-opening calls
/// are replaced by refusals, and `time`, whose `sleep` is replaced by the clamped one. See
/// [`prepare`].
const ALLOWED_NATIVE_MODULES: &[&str] = &[
    // The interpreter's own machinery, which nothing runs without.
    "_abc",
    "_ast",
    "_codecs",
    "_collections",
    "_functools",
    "_imp",
    "_io",
    "_operator",
    // The opcode table `dis` and `inspect` read (`traceback` imports both). Pure data.
    "_opcode",
    "_sre",
    "_stat",
    "_string",
    // The tokenizer the standard library's own source tools use (`tokenize`, which
    // `traceback` imports). Pure text processing.
    "_tokenize",
    "_types",
    "_typing",
    "_warnings",
    "_weakref",
    "atexit",
    "errno",
    "gc",
    "itertools",
    "marshal",
    "time",
    // Arithmetic, text, hashing and data structures: computation, and nothing else.
    "_bisect",
    "_blake2",
    "_contextvars",
    "_csv",
    "_datetime",
    "_hashlib",
    "_heapq",
    "_json",
    "_md5",
    "_queue",
    "_random",
    "_sha1",
    "_sha2",
    "_sha256",
    "_sha3",
    "_sha512",
    "_statistics",
    "_struct",
    "array",
    "binascii",
    "cmath",
    "math",
    "unicodedata",
    "zlib",
];

/// The modules this emulator SUPPLIES in place of ones it does not register: the host
/// functions the runtime source calls.
const SUPPLIED_MODULES: &[&str] = &[HOST_MODULE, "posix"];

/// How many trace events pass between two readings of the whole PROCESS's resident size.
/// That reading is a request to the operating system, so it is taken often enough to catch
/// a process-wide growth inside one bounded stretch of work and seldom enough not to
/// dominate the run; the RUN's own memory is sampled at every event ([`Bounds::step`]).
const STEPS_BETWEEN_PROCESS_SAMPLES: u64 = 2_048;

/// The size of a page of this process's memory, which the faulted-page count is scaled by.
const PAGE_BYTES: u64 = 4_096;

/// The resident size this WHOLE process may reach while a handler is running, whatever any
/// one run has added. An absolute ceiling, so no number of concurrent runs can take the
/// emulator past it.
const PROCESS_RESIDENT_CEILING_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// One invocation to run: the package, the handler, and everything the runtime reports to
/// it.
#[derive(Debug)]
pub struct Run<'a> {
    /// The deployment package's modules.
    pub modules: &'a [PackageModule],
    /// The `Handler` configured for the function (`module.attribute`).
    pub handler: &'a str,
    /// The environment the invocation runs with: the function's own variables plus the
    /// ones the runtime sets.
    pub environment: &'a BTreeMap<String, String>,
    /// The event payload, as the caller sent it.
    pub event: Option<&'a str>,
    /// The invocation's request id.
    pub request_id: &'a str,
    /// The ARN the invocation addressed, as `context.invoked_function_arn`.
    pub invoked_function_arn: &'a str,
    /// The `ClientContext` the caller passed, already decoded to its JSON text.
    pub client_context: Option<&'a str>,
    /// The `TenantId` the caller passed.
    pub tenant_id: Option<&'a str>,
    /// What this run may spend.
    pub budget: Budget,
    /// How the handler reaches the rest of this emulator through the AWS SDK this runtime
    /// supplies it, or `None` for a run with no seam to dispatch a call on — whose
    /// `boto3` reports every call as one that was not made.
    pub aws: Option<&'a AwsAccess>,
}

/// Run `request`'s handler to completion.
///
/// Called on a thread of its own with no state guard of this service held, because the
/// interpreter is synchronous and CPU-bound.
#[must_use]
pub fn run(request: &Run<'_>) -> Finished {
    let started = Instant::now();
    let capture = Arc::new(Mutex::new(Capture::new(request.budget.log_bytes)));
    let bounds = Arc::new(Bounds::new(&request.budget));
    let completion = evaluate(request, &capture, &bounds);
    let elapsed = started.elapsed();
    // The stop LATCH wins over whatever the guest answered: a run whose budget was spent
    // is a stopped run even if the guest caught the exception the hook raised.
    let completion = match bounds.stopped() {
        Some(stop) => Completion::Stopped(stop),
        None => completion,
    };
    let log = captured(&capture).text.clone();
    Finished {
        completion,
        log,
        elapsed,
    }
}

/// Build the interpreter, install the host surface, and drive the runtime source.
fn evaluate(request: &Run<'_>, capture: &Arc<Mutex<Capture>>, bounds: &Arc<Bounds>) -> Completion {
    let input = match runtime_input(request) {
        Ok(input) => input,
        Err(error) => return Completion::Internal(error),
    };
    let contained = Arc::new(AtomicBool::new(false));
    let interpreter = interpreter(&contained);
    // A run only happens when the native surface really was narrowed to the allow-list. If
    // it could not be, nothing is run at all: an uncontained interpreter is never handed
    // guest code.
    if !contained.load(Ordering::SeqCst) {
        return Completion::Internal(
            "the interpreter's native modules could not be narrowed to the allow-list".to_string(),
        );
    }
    let completion = interpreter.enter(|vm| {
        match prepare(vm, request, capture, bounds).and_then(|()| drive(vm, &input, bounds)) {
            Ok(result) => result,
            Err(exception) => Completion::Internal(formatted_exception(vm, &exception)),
        }
    });
    // The interpreter is FINALIZED once the run is over, which is what releases the objects
    // it built: an interpreter that is only dropped leaves its own reference cycles — the
    // module table, the code objects, the exception types — uncollected, so each invocation
    // would hold its interpreter's memory for the life of this process however short the run
    // was. Finalizing it is also what the language itself does at the end of a program, so a
    // handler's `atexit` callback runs the way it runs on the real runtime, and it runs
    // against the SAME bounds the handler did (the step hook is still installed and its stop
    // is latched), so nothing of the run outlasts the run's budget.
    interpreter.finalize(None);
    completion
}

/// The interpreter itself: the frozen standard library and the ALLOW-LISTED native
/// modules, and nothing else.
///
/// `contained` reports whether the narrowing really happened: the module table is reached
/// through the one mutable borrow the initialisation hook has, and a run is refused rather
/// than served if that borrow is not available.
fn interpreter(contained: &Arc<AtomicBool>) -> Interpreter {
    let mut settings = Settings::default();
    // No host path is searched for a module: the standard library is the frozen one and
    // the package's modules are resolved by the runtime's own finder.
    settings.path_list = Vec::new();
    settings.ignore_environment = true;
    settings.isolated = true;
    settings.write_bytecode = false;
    settings.quiet = true;
    // Nothing of the HOST is processed on the way up: no signal handler is installed for
    // this process, and the `site` module — which exists to read host directories — is not
    // imported.
    settings.install_signal_handlers = false;
    settings.import_site = false;
    let contained = Arc::clone(contained);
    Interpreter::with_init(settings, move |vm| {
        let allowed: Vec<&'static ::rustpython_vm::builtins::PyModuleDef> =
            ::rustpython_stdlib::stdlib_module_defs(&vm.ctx)
                .into_iter()
                .filter(|def| ALLOWED_NATIVE_MODULES.contains(&def.name.as_str()))
                .collect();
        // The `posix` module a run is built on is THIS crate's (see [`super::host_posix`]):
        // the import system requires a module of that name to exist, and the engine's own
        // — whose calls are the host's — needs an engine feature this crate does not build
        // at all, so there is none in the binary to register.
        let posix = super::host_posix::module::module_def(&vm.ctx);
        let Some(state) = ::std::rc::Rc::get_mut(&mut vm.state) else {
            return;
        };
        // The native surface of the interpreter, narrowed to the allow-list: the engine's
        // own core modules that are not on it — every one whose calls land on the host —
        // are dropped from the table, so no guest import of them can succeed.
        state
            .module_defs
            .retain(|name, _| ALLOWED_NATIVE_MODULES.contains(name));
        for def in allowed {
            state.module_defs.insert(def.name.as_str(), def);
        }
        state.module_defs.insert("posix", posix);
        state.frozen.extend(::rustpython_pylib::FROZEN_STDLIB);
        contained.store(true, Ordering::SeqCst);
    })
}

/// Install everything the guest sees of the host, before any guest code runs: the modules
/// this emulator supplies in place of the ones it does not register, the refusals that
/// replace the host-reaching calls of the two it does, the bounds, and the log the
/// handler's output goes into.
fn prepare(
    vm: &VirtualMachine,
    request: &Run<'_>,
    capture: &Arc<Mutex<Capture>>,
    bounds: &Arc<Bounds>,
) -> PyResult<()> {
    let modules = vm.sys_module.as_object().get_attr("modules", vm)?;
    // Every allow-listed native module is created ONCE, here, so each is the single
    // patched object `sys.modules` holds from now on. Nothing is imported from a host
    // path: these are the interpreter's own built-in modules.
    for name in ALLOWED_NATIVE_MODULES.iter().chain(["posix"].iter()) {
        let _ = vm.import(*name, 0);
    }
    refuse_host_file_access(vm)?;
    narrow_posix(vm)?;
    install_environment(vm, request.environment)?;
    refuse_guest_threads(vm)?;
    clamp_guest_sleep(vm, bounds)?;
    // Put the host module in `sys.modules` under the name the runtime source imports it
    // by. The import machinery answers from there first, and the name is NOT in the native
    // module table ([`ALLOWED_NATIVE_MODULES`]), so there is no other module of that name
    // to be had: a guest that drops this one cannot import a second.
    modules.set_item(
        HOST_MODULE,
        host_module(vm, capture, bounds, request.aws).into(),
        vm,
    )?;
    // A built-in module can no longer be CREATED: every one the guest can reach is the
    // patched object already in `sys.modules`, and a guest that drops one cannot build
    // itself an unpatched replacement. `is_builtin` stays, because the import system asks
    // it on every import; what is taken away is the creation itself.
    let importer = vm.import("_imp", 0)?;
    let refusing = vm.new_function("create_builtin", |_args: FuncArgs, vm: &VirtualMachine| {
        Err::<PyObjectRef, _>(vm.new_import_error(
            "a Lambda function in this emulator cannot create a built-in module".to_owned(),
            vm.ctx.new_str("<builtin>"),
        ))
    });
    for name in [
        "create_builtin",
        "exec_builtin",
        "create_dynamic",
        "exec_dynamic",
    ] {
        let _ = importer.set_attr(name, refusing.clone(), vm);
    }
    // `os` decides which operating-system module to build itself on from this tuple, so it
    // names the one this emulator supplies.
    let names: Vec<PyObjectRef> = SUPPLIED_MODULES
        .iter()
        .chain(ALLOWED_NATIVE_MODULES.iter())
        .map(|name| vm.ctx.new_str(*name).into())
        .collect();
    vm.sys_module
        .as_object()
        .set_attr("builtin_module_names", vm.ctx.new_tuple(names), vm)?;
    clamp_recursion(vm, request.budget.recursion_limit)
}

/// Install the invocation's own environment into the module `os` builds `os.environ` out
/// of.
///
/// Done HERE rather than from the runtime source, so the guest's environment is the
/// host's statement of it and no Python code of this emulator's own needs an
/// operating-system module. The mapping is the one [`super::host_posix`] created for this
/// interpreter and starts EMPTY, so what a handler reads is the function's own variables
/// and nothing of this process's environment. The entries are bytes, which is the form
/// `os` decodes a POSIX environment from.
fn install_environment(vm: &VirtualMachine, variables: &BTreeMap<String, String>) -> PyResult<()> {
    let system = vm.import("posix", 0)?;
    let environ = system.get_attr("environ", vm)?;
    for (name, value) in variables {
        let key: PyObjectRef = vm.ctx.new_bytes(name.as_bytes().to_vec()).into();
        let held: PyObjectRef = vm.ctx.new_bytes(value.as_bytes().to_vec()).into();
        environ.set_item(&*key, held, vm)?;
    }
    Ok(())
}

/// Replace every call that would open a HOST file with a refusal, on each of the three
/// objects a Python program reaches one through.
///
/// The guest sees the names it expects — `open`, `io.open`, `_io.FileIO` — and the call
/// raises, so no host path resolves for it. The rest of `_io` (the in-memory streams the
/// standard library is built on) is left exactly as it is.
fn refuse_host_file_access(vm: &VirtualMachine) -> PyResult<()> {
    // `io` is loaded FIRST, so the standard library's own stream classes are bound and
    // registered from the real module before anything on it is replaced.
    let _ = vm.import("io", 0);
    let refusing = vm.new_function("open", |_args: FuncArgs, vm: &VirtualMachine| {
        Err::<PyObjectRef, _>(vm.new_os_error(
            "a Lambda function in this emulator cannot open a file of the host".to_owned(),
        ))
    });
    let targets: [(&str, &[&str]); 2] = [
        ("_io", &["open", "open_code", "FileIO"]),
        ("io", &["open", "open_code", "FileIO"]),
    ];
    for (module, attributes) in targets {
        if let Ok(module) = vm.import(module, 0) {
            for attribute in attributes {
                let _ = module.set_attr(*attribute, refusing.clone(), vm);
            }
        }
    }
    let builtins = vm.builtins.as_object();
    for attribute in ["open", "open_code"] {
        let _ = builtins.set_attr(attribute, refusing.clone(), vm);
    }
    Ok(())
}

/// Replace the interpreter's own `sleep` with one clamped to what the run has left, on the
/// single clock module `sys.modules` holds.
fn clamp_guest_sleep(vm: &VirtualMachine, bounds: &Arc<Bounds>) -> PyResult<()> {
    let sleep_bounds = Arc::clone(bounds);
    let clamped = vm.new_function("sleep", move |args: FuncArgs, vm: &VirtualMachine| {
        let seconds: f64 = args.bind(vm)?;
        bounded_sleep(&sleep_bounds, seconds);
        Ok::<PyObjectRef, ::rustpython_vm::builtins::PyBaseExceptionRef>(vm.ctx.none())
    });
    if let Ok(clock) = vm.import("time", 0) {
        let _ = clock.set_attr("sleep", clamped, vm);
    }
    Ok(())
}

/// Wait for `seconds`, but never past what the run has left: a blocking wait the guest
/// asked for cannot outlast the run's deadline.
///
/// A value that is not a finite number of seconds is no wait at all, so nothing is waited
/// for: the guest decides how long to ask for, never how the host reads it.
fn bounded_sleep(bounds: &Arc<Bounds>, seconds: f64) {
    if !seconds.is_finite() || seconds <= 0.0 {
        return;
    }
    let asked = Duration::from_secs_f64(seconds.min(MAX_GUEST_SLEEP_SECONDS));
    ::std::thread::sleep(asked.min(bounds.remaining()));
}

/// The longest a guest may ask to wait for in one call, before the run's own remaining
/// time clamps it further.
const MAX_GUEST_SLEEP_SECONDS: f64 = 3_600.0;

/// Evaluate the runtime source and read its answer back.
///
/// The runtime's own setup — its imports and definitions — runs FIRST, with no bound
/// spending against it, because none of it is the caller's work. The bounds are installed
/// between that and the one call that does the caller's work, so the step budget, the
/// deadline and the memory sample all measure the HANDLER.
fn drive(vm: &VirtualMachine, input: &str, bounds: &Arc<Bounds>) -> PyResult<Completion> {
    let scope = vm.new_scope_with_builtins();
    let code = vm
        .compile(
            super::bootstrap::RUNTIME_SOURCE,
            ::rustpython_vm::compiler::Mode::Exec,
            "<lambda-runtime>".to_owned(),
        )
        .map_err(|error| {
            vm.new_runtime_error(format!("the Lambda runtime source does not compile: {error}"))
        })?;
    vm.run_code_obj(code, scope.clone())?;
    let entry = scope.globals.get_item("__lambda_entry", vm)?;
    install_trace_hook(vm)?;
    bounds.start();
    let answered = entry.call((vm.ctx.new_str(input),), vm)?;
    let text: String = answered.try_into_value(vm)?;
    Ok(read_completion(&text, bounds.response_bytes))
}

/// The runtime's answer, as this module reports it.
fn read_completion(answered: &str, response_bytes: usize) -> Completion {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(answered) else {
        return Completion::Internal("the runtime answered with a document that is not JSON".into());
    };
    let member = |name: &str| {
        document
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match document.get("outcome").and_then(::serde_json::Value::as_str) {
        Some("returned") => {
            let payload = member("payload");
            if payload.len() > response_bytes {
                return Completion::Stopped(Stop::Response);
            }
            Completion::Returned(payload)
        }
        Some("raised") => Completion::Raised {
            error_type: member("errorType"),
            error_message: member("errorMessage"),
            stack_trace: document
                .get("stackTrace")
                .and_then(::serde_json::Value::as_array)
                .map(|frames| {
                    frames
                        .iter()
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        },
        _ => Completion::Internal("the runtime answered with no outcome".into()),
    }
}

/// The JSON document the runtime source is handed.
fn runtime_input(request: &Run<'_>) -> Result<String, String> {
    let modules: ::serde_json::Map<String, ::serde_json::Value> = request
        .modules
        .iter()
        .map(|module| {
            (
                module.path.clone(),
                ::serde_json::Value::String(module.source.clone()),
            )
        })
        .collect();
    let environment: ::serde_json::Map<String, ::serde_json::Value> = request
        .environment
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    let client_context = match request.client_context {
        Some(text) => ::serde_json::from_str::<::serde_json::Value>(text)
            .unwrap_or(::serde_json::Value::Null),
        None => ::serde_json::Value::Null,
    };
    let deadline = SystemTime::now() + request.budget.wall_clock;
    let deadline_epoch_ms = deadline
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    let document = ::serde_json::json!({
        "handler": request.handler,
        "modules": modules,
        "runtimeModules": runtime_modules(),
        "environment": environment,
        "event": request.event,
        "requestId": request.request_id,
        "invokedFunctionArn": request.invoked_function_arn,
        "clientContext": client_context,
        "tenantId": request.tenant_id,
        "deadlineEpochMs": deadline_epoch_ms,
    });
    ::serde_json::to_string(&document).map_err(|error| error.to_string())
}

/// The modules this RUNTIME supplies on the handler's import path, beside the deployment
/// package's own: the AWS SDK a managed `python3.x` runtime ships
/// ([`super::aws_sdk_guest`]).
fn runtime_modules() -> ::serde_json::Map<String, ::serde_json::Value> {
    super::aws_sdk_guest::runtime_modules()
        .into_iter()
        .map(|module| (module.path, ::serde_json::Value::String(module.source)))
        .collect()
}

/// Install the trace hook the step and deadline bounds are checked from, then take away
/// every way the guest could clear or replace it.
fn install_trace_hook(vm: &VirtualMachine) -> PyResult<()> {
    let scope = vm.new_scope_with_builtins();
    let code = vm
        .compile(
            TRACE_HOOK_SOURCE,
            ::rustpython_vm::compiler::Mode::Exec,
            "<lambda-bounds>".to_owned(),
        )
        .map_err(|error| {
            vm.new_runtime_error(format!("the bounds hook source does not compile: {error}"))
        })?;
    vm.run_code_obj(code, scope)?;
    // With these gone, no guest call can reach the interpreter's trace slot at all: the
    // hook that spends the budget is the only one there will be for this run.
    for name in ["settrace", "setprofile", "gettrace", "getprofile"] {
        let _ = vm.sys_module.as_object().del_attr(name, vm);
    }
    Ok(())
}

/// The hook itself. A trace function has to be a Python callable that returns the local
/// tracer to use, so this is three lines of Python over the host's own step hook; the
/// budget it spends, and the exception it raises, are [`Bounds::step`]'s.
const TRACE_HOOK_SOURCE: &str = r#"
import sys
_host = __import__("_lambda_host")

def __lambda_trace(frame, event, arg):
    _host.step()
    return __lambda_trace

sys.settrace(__lambda_trace)
"#;

/// Clamp how deep the guest may recurse, and take away the call that would raise it.
fn clamp_recursion(vm: &VirtualMachine, limit: usize) -> PyResult<()> {
    let setter = vm.sys_module.as_object().get_attr("setrecursionlimit", vm)?;
    setter.call((limit,), vm)?;
    let _ = vm.sys_module.as_object().del_attr("setrecursionlimit", vm);
    Ok(())
}

/// The module the four host functions live on: the invocation log, the bounded sleep, the
/// step hook and the one AWS SDK call. Nothing else of the host is reachable from guest
/// code.
fn host_module(
    vm: &VirtualMachine,
    capture: &Arc<Mutex<Capture>>,
    bounds: &Arc<Bounds>,
    aws: Option<&AwsAccess>,
) -> PyRef<PyModule> {
    let module = vm.new_module(HOST_MODULE, vm.ctx.new_dict(), None);
    let log_capture = Arc::clone(capture);
    set_function(vm, &module, "log", move |args: FuncArgs, vm: &VirtualMachine| {
        let text: String = args.bind(vm)?;
        captured(&log_capture).write(&text);
        Ok(vm.ctx.none())
    });
    let sleep_bounds = Arc::clone(bounds);
    set_function(
        vm,
        &module,
        "sleep",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let seconds: f64 = args.bind(vm)?;
            // A sleep the guest asked for never outlasts the run: it is clamped to what
            // the run has left, so no guest call can wait past the deadline.
            bounded_sleep(&sleep_bounds, seconds);
            Ok(vm.ctx.none())
        },
    );
    let step_bounds = Arc::clone(bounds);
    set_function(
        vm,
        &module,
        "step",
        move |_args: FuncArgs, vm: &VirtualMachine| match step_bounds.step() {
            None => Ok(vm.ctx.none()),
            Some(stop) => Err(vm.new_runtime_error(stop.reason().to_owned())),
        },
    );
    // The ONE call a handler reaches another service of this emulator by. The service
    // table, the signature and the destination are all [`super::aws_api`]'s, so what the
    // guest decides is only which operation of which reachable service it asks for and
    // what it sends; the two bounds that are this run's — how many calls it may make and
    // how long one may take — are held here, where no guest code can reach them.
    let access = aws.cloned();
    let call_bounds = Arc::clone(bounds);
    let made = Arc::new(AtomicU64::new(0));
    set_function(
        vm,
        &module,
        "aws",
        move |args: FuncArgs, vm: &VirtualMachine| {
            let (service, operation, region, body): (String, String, String, String) =
                args.bind(vm)?;
            // A run whose budget is already spent makes no further call: the latched stop
            // is its answer, so work it starts afterwards is work nothing will report.
            if let Some(stop) = call_bounds.stopped() {
                return Err(vm.new_runtime_error(stop.reason().to_owned()));
            }
            if made.fetch_add(1, Ordering::Relaxed) >= MAX_CALLS_PER_RUN as u64 {
                return Err(vm.new_runtime_error(format!(
                    "one invocation may make at most {MAX_CALLS_PER_RUN} AWS SDK calls"
                )));
            }
            let Some(access) = access.as_ref() else {
                return Ok(vm
                    .ctx
                    .new_str(refusal_answer(
                        "not_made",
                        "this invocation was given no AWS SDK seam, so no call was made",
                    ))
                    .into());
            };
            let remaining = call_bounds.remaining();
            if remaining.is_zero() {
                return Err(vm.new_runtime_error(Stop::Deadline.reason().to_owned()));
            }
            let answered = access.call(&service, &operation, &region, &body, remaining);
            Ok(vm.ctx.new_str(call_answer(&answered)).into())
        },
    );
    module
}

/// What the guest reads back from one AWS SDK call: the service's own answer, or the
/// reason no call was made.
///
/// A document rather than a Python object, for the reason the runtime's own protocol is one
/// ([`super::bootstrap`]): the host's interpreter surface stays a string in and a string
/// out.
fn call_answer(answered: &Result<super::aws_api::Answer, Refusal>) -> String {
    let document = match answered {
        Ok(answer) => ::serde_json::json!({
            "status": answer.status,
            "body": answer.body,
            "errorType": answer.error_type,
            "requestId": answer.request_id,
        }),
        Err(Refusal::UnknownService) => {
            return refusal_answer(
                "unknown_service",
                &super::aws_api::reachable_services().join(", "),
            )
        }
        Err(Refusal::UnknownOperation) => return refusal_answer("unknown_operation", ""),
        Err(Refusal::TimedOut) => return refusal_answer("timed_out", ""),
        Err(Refusal::RequestTooLarge) => {
            return refusal_answer(
                "not_made",
                &format!(
                    "the request document is larger than the {} bytes one AWS SDK call may \
                     carry",
                    super::aws_api::MAX_REQUEST_BYTES
                ),
            )
        }
        Err(Refusal::ResponseTooLarge) => {
            return refusal_answer(
                "not_made",
                &format!(
                    "the answer is larger than the {} bytes one AWS SDK call may read back",
                    super::aws_api::MAX_RESPONSE_BYTES
                ),
            )
        }
        Err(Refusal::NotMade(detail)) => return refusal_answer("not_made", detail),
    };
    document.to_string()
}

/// The answer for a call that was not made, naming which refusal it was.
fn refusal_answer(refused: &str, detail: &str) -> String {
    ::serde_json::json!({ "refused": refused, "detail": detail }).to_string()
}

/// The names Python's `os` looks for on the module it is built on, every one of which
/// lands on the HOST where a real runtime answers it: a file, a directory, a descriptor, a
/// process, a user or the host clock's own accounting.
///
/// Each one EXISTS on this emulator's `posix` (`os` builds itself out of the names it
/// finds, so an absent one changes what `os` offers) and each one RAISES when it is
/// called: a Lambda function in this emulator has no host filesystem and no host process
/// to reach. Nothing here is a stub that pretends to have done the work.
const REFUSED_POSIX_CALLS: &[&str] = &[
    "access", "chdir", "chmod", "chown", "close", "closerange", "confstr", "cpu_count",
    "device_encoding", "dup", "dup2",
    "execv", "execve", "fchdir", "fchmod", "fchown", "fdatasync", "fdopen", "fork", "forkpty",
    "fstat", "fstatvfs", "fsync", "ftruncate", "get_blocking", "get_inheritable",
    "get_terminal_size", "getegid", "geteuid", "getgid", "getgrouplist", "getgroups", "getlogin",
    "getppid", "getpriority", "getsid", "getuid", "isatty", "kill", "killpg", "lchown", "link",
    "listdir", "lseek", "lstat", "major", "makedev", "minor", "mkdir", "mkfifo", "mknod", "nice",
    "open", "openpty", "pipe", "popen", "pread", "putenv_host", "pwrite", "read", "readlink",
    "readv", "remove", "rename", "replace", "rmdir", "scandir", "sched_yield", "sendfile",
    "set_blocking", "set_inheritable", "setegid", "seteuid", "setgid", "setgroups", "setpgid",
    "setpgrp", "setpriority", "setregid", "setreuid", "setsid", "setuid", "spawnv", "spawnve",
    "pathconf", "fpathconf", "process_cpu_count", "sched_getaffinity", "sched_setaffinity",
    "sysconf", "waitstatus_to_exitcode", "lockf", "getrandom",
    "stat", "statvfs", "symlink", "sync", "system", "tcgetpgrp", "tcsetpgrp", "times", "truncate",
    "ttyname", "umask", "uname", "unlink", "utime", "wait", "wait3", "wait4", "waitpid", "write",
    "writev",
];

/// Put a refusal on `posix` under every name of [`REFUSED_POSIX_CALLS`].
///
/// The module itself is this crate's ([`super::host_posix`]) and carries only the calls a
/// run can be answered from its own memory; these are the names `os` looks for beyond
/// those, so that `os` is built out of a complete surface and every one of them raises.
fn narrow_posix(vm: &VirtualMachine) -> PyResult<()> {
    let system = vm.import("posix", 0)?;
    // `os` builds the sets that say which of its calls take a directory descriptor or
    // follow symbolic links out of this list. It is EMPTY, which is what says that none of
    // them does: there is no host descriptor and no host link to follow.
    let _ = system.set_attr("_have_functions", vm.ctx.new_list(Vec::new()), vm);
    for name in REFUSED_POSIX_CALLS {
        let refusing = vm.new_function(name, |_args: FuncArgs, vm: &VirtualMachine| {
            Err::<PyObjectRef, _>(vm.new_os_error(
                "a Lambda function in this emulator has no host filesystem or process to reach"
                    .to_owned(),
            ))
        });
        let _ = system.set_attr(*name, refusing, vm);
    }
    Ok(())
}

/// Take away every call that would start a HOST thread, on the single `_thread` module
///
/// A real guest thread would run outside the step budget, the stop latch and the memory
/// sample, so none can be started. The build registers no native thread module at all (the
/// engine feature that would is not taken, see this crate's `Cargo.toml`), so the one a
/// guest imports is the engine's own thread-less stub; these refusals are what make that
/// explicit at every name that starts something.
fn refuse_guest_threads(vm: &VirtualMachine) -> PyResult<()> {
    let refusing = vm.new_function(
        "start_new_thread",
        |_args: FuncArgs, vm: &VirtualMachine| {
            Err::<PyObjectRef, _>(vm.new_runtime_error(
                "a Lambda function in this emulator cannot start a thread".to_owned(),
            ))
        },
    );
    if let Ok(threads) = vm.import("_thread", 0) {
        for attribute in [
            "start_new_thread",
            "start_new",
            "start_joinable_thread",
            "interrupt_main",
            "_set_sentinel",
        ] {
            let _ = threads.set_attr(attribute, refusing.clone(), vm);
        }
    }
    Ok(())
}

/// Put one host function on `module` under `name`.
fn set_function<F>(
    vm: &VirtualMachine,
    module: &PyRef<PyModule>,
    name: &'static str,
    function: F,
) where
    F: Fn(FuncArgs, &VirtualMachine) -> PyResult<PyObjectRef> + Send + Sync + 'static,
{
    let callable = vm.new_function(name, function);
    let _ = module.as_object().set_attr(name, callable, vm);
}

/// How an exception of the interpreter ITSELF is reported — never to a caller as a result,
/// only as the internal failure it is.
fn formatted_exception(
    vm: &VirtualMachine,
    exception: &::rustpython_vm::builtins::PyBaseExceptionRef,
) -> String {
    let mut rendered = String::new();
    if vm.write_exception(&mut rendered, exception).is_err() {
        rendered.push_str("an exception that could not be rendered");
    }
    rendered
}

/// The run's budget, as the hook spends it. Shared with the host functions and read by the
/// caller after the run.
#[derive(Debug)]
pub struct Bounds {
    /// When the run must be over. Set when the handler's own work starts.
    deadline: Mutex<Instant>,
    /// How long the handler may run for.
    wall_clock: Duration,
    /// How many traced events are left.
    steps: AtomicU64,
    /// How many have been spent, for the memory-sample interval.
    spent: AtomicU64,
    /// How much memory the run may add to this process.
    memory_bytes: u64,
    /// How large the handler's response may be.
    response_bytes: usize,
    /// This thread's faulted bytes when the handler's work started.
    faulted_at_start: AtomicU64,
    /// Which bound was breached, once one was. Latched: the answer to a run that spent its
    /// budget is that stop, whatever the guest does afterwards.
    stopped: Mutex<Option<Stop>>,
    /// Whether a bound was breached at all, read without taking the lock.
    breached: AtomicBool,
}

impl Bounds {
    /// The bounds of a run with `budget`. The clock and the memory sample they measure
    /// against are taken by [`Self::start`], when the handler's own work begins.
    fn new(budget: &Budget) -> Self {
        Self {
            deadline: Mutex::new(Instant::now() + budget.wall_clock),
            wall_clock: budget.wall_clock,
            steps: AtomicU64::new(budget.steps),
            spent: AtomicU64::new(0),
            memory_bytes: budget.memory_bytes,
            response_bytes: budget.response_bytes,
            faulted_at_start: AtomicU64::new(faulted_bytes()),
            stopped: Mutex::new(None),
            breached: AtomicBool::new(false),
        }
    }

    /// Start spending: the deadline runs from HERE and the memory sample is taken from
    /// here, so what the bounds measure is the handler's own work rather than the
    /// interpreter this emulator built to run it in.
    fn start(&self) {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) =
            Instant::now() + self.wall_clock;
        self.faulted_at_start
            .store(faulted_bytes(), Ordering::Relaxed);
    }

    /// When the run must be over.
    fn deadline(&self) -> Instant {
        *self.deadline.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Spend one traced event: `None` to carry on, `Some(stop)` for the bound that is now
    /// breached.
    fn step(&self) -> Option<Stop> {
        if let Some(stop) = self.stopped() {
            return Some(stop);
        }
        if self.steps.fetch_sub(1, Ordering::Relaxed) == 0 {
            return Some(self.breach(Stop::Steps));
        }
        if Instant::now() >= self.deadline() {
            return Some(self.breach(Stop::Deadline));
        }
        // The run's OWN memory is sampled at EVERY traced event, so there is no sampling
        // interval for an allocating guest to outrun: the thread's faulted pages are a
        // counter this process already keeps. The whole process's resident size is read
        // from the operating system, which is the dearer of the two, so that one is read
        // every [`STEPS_BETWEEN_PROCESS_SAMPLES`] events.
        let spent = self.spent.fetch_add(1, Ordering::Relaxed);
        if self.past_run_ceiling() {
            return Some(self.breach(Stop::Memory));
        }
        if spent.is_multiple_of(STEPS_BETWEEN_PROCESS_SAMPLES)
            && resident_bytes() > PROCESS_RESIDENT_CEILING_BYTES
        {
            return Some(self.breach(Stop::Memory));
        }
        None
    }

    /// Whether the run has grown past its own ceiling, sampled from OUTSIDE the guest: the
    /// interpreter exposes no allocation seam, so the bound is read from this thread's
    /// faulted pages.
    fn past_run_ceiling(&self) -> bool {
        faulted_bytes().saturating_sub(self.faulted_at_start.load(Ordering::Relaxed))
            > self.memory_bytes
    }

    /// Latch `stop` as the run's answer and report it.
    fn breach(&self, stop: Stop) -> Stop {
        let mut latched = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let already = *latched.get_or_insert(stop);
        self.breached.store(true, Ordering::Relaxed);
        already
    }

    /// The bound this run breached, if it breached one.
    #[must_use]
    pub fn stopped(&self) -> Option<Stop> {
        if !self.breached.load(Ordering::Relaxed) {
            return None;
        }
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How long the run has left.
    fn remaining(&self) -> Duration {
        self.deadline().saturating_duration_since(Instant::now())
    }
}

/// How many bytes of memory THIS thread has faulted in, as the host reports it.
fn faulted_bytes() -> u64 {
    match ::nix::sys::resource::getrusage(::nix::sys::resource::UsageWho::RUSAGE_THREAD) {
        Ok(usage) => {
            let pages = usage.minor_page_faults().saturating_add(usage.major_page_faults());
            u64::try_from(pages).unwrap_or(0).saturating_mul(PAGE_BYTES)
        }
        Err(_) => 0,
    }
}

/// How many bytes this PROCESS holds resident, as the host reports it.
fn resident_bytes() -> u64 {
    match ::memory_stats::memory_stats() {
        Some(stats) => u64::try_from(stats.physical_mem).unwrap_or(0),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A budget small enough to exhaust in a test and large enough for any handler one
    /// runs.
    fn budget() -> Budget {
        Budget {
            wall_clock: Duration::from_secs(5),
            steps: 2_000_000,
            memory_bytes: 256 * 1024 * 1024,
            log_bytes: 16 * 1024,
            response_bytes: 64 * 1024,
            recursion_limit: 2_000,
        }
    }

    /// Run `source` as the module `lambda_function` of a one-file package, on a thread with
    /// the stack a run is given — the same way [`super::run_invocation`] runs one.
    fn ran(source: &str, handler: &str, event: Option<&str>) -> Finished {
        ran_with(source, handler, event, &BTreeMap::new(), budget())
    }

    /// [`ran`] with an environment and a budget of its own.
    fn ran_with(
        source: &str,
        handler: &str,
        event: Option<&str>,
        environment: &BTreeMap<String, String>,
        budget: Budget,
    ) -> Finished {
        let modules = vec![PackageModule {
            path: "lambda_function.py".to_string(),
            source: source.to_string(),
        }];
        let handler = handler.to_string();
        let event = event.map(str::to_string);
        let environment = environment.clone();
        ::std::thread::Builder::new()
            .stack_size(crate::state::execution::RUN_STACK_BYTES)
            .spawn(move || {
                run(&Run {
                    modules: &modules,
                    handler: &handler,
                    environment: &environment,
                    event: event.as_deref(),
                    request_id: "11111111-2222-3333-4444-555555555555",
                    invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                    client_context: None,
                    tenant_id: None,
                    budget,
                    // These runs are the interpreter's own containment and bounds: no
                    // emulator is standing behind them, so there is no SDK seam either.
                    aws: None,
                })
            })
            .expect("a run thread")
            .join()
            .expect("the run thread ended")
    }

    /// The JSON the handler returned, or a panic naming what it came to instead.
    fn returned(finished: &Finished, case: &str) -> ::serde_json::Value {
        match &finished.completion {
            Completion::Returned(payload) => ::serde_json::from_str(payload)
                .unwrap_or_else(|_| panic!("case {case}: expected JSON, got {payload:?}")),
            other => panic!("case {case}: expected a returned value, got {other:?}"),
        }
    }

    /// The handler's own value is what comes back, for one row per JSON type a handler can
    /// return — so the answer is the run's, not a shape this emulator fills in.
    #[test]
    fn the_answer_is_what_the_handler_returned() {
        let cases: &[(&str, &str, &str)] = &[
            ("a bool member", "{'ok': True}", r#"{"ok":true}"#),
            ("a computed number", "{'n': sum(range(10))}", r#"{"n":45}"#),
            ("a string", "'hello'", r#""hello""#),
            ("a list", "[1, 2, 3]", "[1,2,3]"),
            ("null", "None", "null"),
            (
                "the event echoed back",
                "{'event': event}",
                r#"{"event":{"k":"v"}}"#,
            ),
            (
                "a value from the standard library",
                "{'encoded': __import__('base64').b64encode(b'ab').decode()}",
                r#"{"encoded":"YWI="}"#,
            ),
            (
                "a value from json itself",
                "{'parsed': __import__('json').loads('[1, 2]')}",
                r#"{"parsed":[1,2]}"#,
            ),
        ];
        for (case, expression, expected) in cases {
            let source = format!("def handler(event, context):\n    return {expression}\n");
            let finished = ran(&source, "lambda_function.handler", Some(r#"{"k": "v"}"#));
            let actual = returned(&finished, case);
            let wanted: ::serde_json::Value =
                ::serde_json::from_str(expected).expect("the expected JSON");
            assert_eq!(
                actual, wanted,
                "case {case}: expected {wanted}, got {actual} (log {:?})",
                finished.log
            );
        }
    }

    /// The context object reports the invocation, out of the environment the run was given.
    #[test]
    fn the_context_reports_the_invocation() {
        let mut environment = BTreeMap::new();
        environment.insert("AWS_LAMBDA_FUNCTION_NAME".to_string(), "probe".to_string());
        environment.insert("AWS_LAMBDA_FUNCTION_VERSION".to_string(), "7".to_string());
        environment.insert(
            "AWS_LAMBDA_FUNCTION_MEMORY_SIZE".to_string(),
            "512".to_string(),
        );
        environment.insert("GREETING".to_string(), "hello".to_string());
        let source = "import os\n\
                      def handler(event, context):\n    \
                          return {\n        \
                              'name': context.function_name,\n        \
                              'version': context.function_version,\n        \
                              'memory': context.memory_limit_in_mb,\n        \
                              'arn': context.invoked_function_arn,\n        \
                              'request': context.aws_request_id,\n        \
                              'left': context.get_remaining_time_in_millis() > 0,\n        \
                              'greeting': os.environ['GREETING'],\n        \
                              'cwd': os.getcwd(),\n    \
                          }\n";
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &environment,
            budget(),
        );
        let answered = returned(&finished, "the context");
        let cases: &[(&str, ::serde_json::Value)] = &[
            ("name", ::serde_json::json!("probe")),
            ("version", ::serde_json::json!("7")),
            ("memory", ::serde_json::json!("512")),
            (
                "arn",
                ::serde_json::json!("arn:aws:lambda:us-west-2:000000000000:function:probe"),
            ),
            (
                "request",
                ::serde_json::json!("11111111-2222-3333-4444-555555555555"),
            ),
            ("left", ::serde_json::json!(true)),
            ("greeting", ::serde_json::json!("hello")),
            ("cwd", ::serde_json::json!("/var/task")),
        ];
        for (member, expected) in cases {
            let actual = answered.get(*member).cloned().unwrap_or_default();
            assert_eq!(
                actual, *expected,
                "case context.{member}: expected {expected}, got {actual} (log {:?})",
                finished.log
            );
        }
    }

    /// What the handler wrote to its output streams is the run's log, and nothing of the
    /// host's.
    #[test]
    fn the_log_is_what_the_handler_wrote() {
        let source = "import sys\n\
                      def handler(event, context):\n    \
                          print('to stdout')\n    \
                          sys.stderr.write('to stderr\\n')\n    \
                          return 1\n";
        let finished = ran(source, "lambda_function.handler", None);
        for expected in ["to stdout", "to stderr"] {
            assert!(
                finished.log.contains(expected),
                "case {expected:?}: expected it in the log, got {:?}",
                finished.log
            );
        }
    }

    /// An exception the handler raised is reported as the exception it is, with the
    /// handler's own stack.
    #[test]
    fn an_exception_the_handler_raised_is_reported_as_itself() {
        let cases: &[(&str, &str, &str)] = &[
            ("raise ValueError('boom')", "ValueError", "boom"),
            ("raise KeyError('k')", "KeyError", "'k'"),
            ("return 1 / 0", "ZeroDivisionError", "division by zero"),
            (
                "return {'set': set()}",
                "Runtime.MarshalError",
                "Unable to marshal response",
            ),
        ];
        for (statement, error_type, message) in cases {
            let source = format!("def handler(event, context):\n    {statement}\n");
            let finished = ran(&source, "lambda_function.handler", None);
            match &finished.completion {
                Completion::Raised {
                    error_type: actual_type,
                    error_message,
                    stack_trace,
                } => {
                    assert_eq!(
                        actual_type, error_type,
                        "case {statement:?}: expected the type {error_type}, got {actual_type}"
                    );
                    assert!(
                        error_message.contains(message),
                        "case {statement:?}: expected {message:?} in the message, got \
                         {error_message:?}"
                    );
                    if !error_type.starts_with("Runtime.") {
                        assert!(
                            stack_trace.iter().any(|frame| frame.contains("/var/task/")),
                            "case {statement:?}: expected the handler's own frame, got \
                             {stack_trace:?}"
                        );
                    }
                }
                other => panic!("case {statement:?}: expected a raise, got {other:?}"),
            }
        }
    }

    /// A handler that cannot be resolved is the runtime failure for it, named as the real
    /// runtime names it — one row per way a handler fails to resolve.
    #[test]
    fn a_handler_that_does_not_resolve_is_reported_as_the_runtime_failure() {
        let source = "def handler(event, context):\n    return 1\n";
        let cases: &[(&str, &str)] = &[
            ("lambda_function.absent", "Runtime.HandlerNotFound"),
            ("absent_module.handler", "Runtime.ImportModuleError"),
            ("handler", "Runtime.MalformedHandlerName"),
        ];
        for (handler, expected) in cases {
            let finished = ran(source, handler, None);
            match &finished.completion {
                Completion::Raised { error_type, .. } => assert_eq!(
                    error_type, expected,
                    "case {handler:?}: expected {expected}, got {error_type}"
                ),
                other => panic!("case {handler:?}: expected a raise, got {other:?}"),
            }
        }
    }

    /// Every bound is the HOST's: one row per bound, each exhausted from guest code, and
    /// each ends the run with that bound rather than with an answer.
    #[test]
    fn a_run_that_breaches_a_bound_is_stopped_by_it() {
        let cases: &[(&str, &str, Stop)] = &[
            (
                "a loop with no exit",
                "while True:\n        pass\n",
                Stop::Steps,
            ),
            (
                "unbounded recursion",
                "def deep(n):\n        return deep(n + 1)\n    deep(0)\n",
                Stop::Steps,
            ),
            (
                "a response larger than one may be",
                "return 'x' * 200000\n",
                Stop::Response,
            ),
            (
                "an allocating loop",
                "kept = []\n    \
                     while True:\n        \
                         kept.append(bytearray(1024 * 1024))\n",
                Stop::Memory,
            ),
        ];
        for (case, body, expected) in cases {
            let source = format!("def handler(event, context):\n    {body}");
            let mut tight = budget();
            tight.steps = 50_000;
            // Small enough that an allocating handler passes it inside the step budget,
            // and far above what building the interpreter itself faults in.
            tight.memory_bytes = 64 * 1024 * 1024;
            let finished = ran_with(
                &source,
                "lambda_function.handler",
                None,
                &BTreeMap::new(),
                tight,
            );
            let stopped = match &finished.completion {
                Completion::Stopped(stop) => Some(*stop),
                // Deep recursion may end at the interpreter's own clamped limit first,
                // which is also the host's bound doing its job.
                Completion::Raised { error_type, .. } if error_type == "RecursionError" => None,
                other => panic!("case {case}: expected a stop, got {other:?}"),
            };
            if let Some(stopped) = stopped {
                assert_eq!(
                    stopped, *expected,
                    "case {case}: expected {expected:?}, got {stopped:?}"
                );
            }
        }
    }

    /// The step budget cannot be escaped by catching what the hook raises: the stop is
    /// LATCHED, so the run's answer is the stop whatever the guest returns afterwards.
    #[test]
    fn a_guest_that_catches_the_stop_is_still_stopped() {
        let source = "def handler(event, context):\n    \
                          try:\n        \
                              while True:\n            \
                                  pass\n    \
                          except BaseException:\n        \
                              return {'escaped': True}\n";
        let mut tight = budget();
        tight.steps = 20_000;
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
        );
        assert!(
            matches!(finished.completion, Completion::Stopped(_)),
            "expected the latched stop to win, got {:?}",
            finished.completion
        );
    }

    /// The deadline is the host's too: a sleep the guest asks for never outlasts the run.
    #[test]
    fn a_sleep_past_the_deadline_does_not_outlast_the_run() {
        let source = "import time\n\
                      def handler(event, context):\n    \
                          time.sleep(600)\n    \
                          return {'slept': True}\n";
        let mut tight = budget();
        tight.wall_clock = Duration::from_secs(2);
        let started = Instant::now();
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &BTreeMap::new(),
            tight,
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "expected the clamped sleep to end inside the run, took {:?}",
            started.elapsed()
        );
        assert!(
            !matches!(finished.completion, Completion::Internal(_)),
            "expected the run to end on its own bound, got {:?}",
            finished.completion
        );
    }

    /// Containment, attempted FROM guest code: one row per host surface a handler must not
    /// reach. Each row is the expression, and each has to fail.
    #[test]
    fn no_host_surface_is_reachable_from_guest_code() {
        let cases: &[(&str, &str)] = &[
            ("a subprocess", "__import__('subprocess')"),
            ("a shell command", "__import__('os').system('echo hi')"),
            ("a process pipe", "__import__('os').popen('echo hi')"),
            ("a host file descriptor", "__import__('os').open('/etc/hostname', 0)"),
            ("a host file removal", "__import__('os').remove('/etc/hostname')"),
            ("a fork", "__import__('os').fork()"),
            ("a socket", "__import__('socket').socket()"),
            ("the host's posix", "__import__('posix').open('/etc/hostname', 0)"),
            ("a host file", "open('/etc/hostname').read()"),
            ("a host directory", "__import__('os').listdir('/')"),
            ("a thread", "__import__('_thread').start_new_thread(print, ())"),
            ("a memory map", "__import__('mmap')"),
            ("a host library", "__import__('ctypes')"),
            ("the host's environment", "__import__('os').environ['PATH']"),
            ("a re-import of a dropped module", "__import__('sys').modules.pop('posix', None) or __import__('posix').open('/etc/hostname', 0)"),
            ("the trace hook", "__import__('sys').settrace(None)"),
            ("the recursion limit", "__import__('sys').setrecursionlimit(1000000)"),
            ("the process", "__import__('os')._exit(0)"),
        ];
        for (case, expression) in cases {
            let source = format!("def handler(event, context):\n    return {expression}\n");
            let finished = ran(&source, "lambda_function.handler", None);
            match &finished.completion {
                Completion::Raised { .. } | Completion::Stopped(_) => {}
                other => panic!(
                    "case {case}: expected guest code reaching {case} to fail, got {other:?} \
                     (log {:?})",
                    finished.log
                ),
            }
        }
    }

    /// The ENVIRONMENT a run sees is the invocation's own and nothing of this process's:
    /// read from inside the run, which is where it matters.
    #[test]
    fn the_environment_a_run_sees_holds_nothing_of_the_host() {
        let mut environment = BTreeMap::new();
        environment.insert("SECRET".to_string(), "shhh".to_string());
        let source = "import os\n\
                      def handler(event, context):\n    \
                          return {'own': os.environ.get('SECRET'), \
                                  'names': sorted(os.environ.keys())}\n";
        let finished = ran_with(
            source,
            "lambda_function.handler",
            None,
            &environment,
            budget(),
        );
        let answered = returned(&finished, "the environment");
        assert_eq!(
            answered.get("own").and_then(::serde_json::Value::as_str),
            Some("shhh"),
            "expected the invocation's own variable, got {answered}"
        );
        let names: Vec<&str> = answered
            .get("names")
            .and_then(::serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["SECRET"],
            "expected only the invocation's own variables, got {names:?}"
        );
    }

    /// The native modules a run can reach are WITHIN the allow-list — read from inside a    /// run, which is where it matters — and no host-reaching one is among them.
    #[test]
    fn the_registered_native_modules_are_within_the_allow_list() {
        let source = "import sys\n\
                      def handler(event, context):\n    \
                          return sorted(sys.builtin_module_names)\n";
        let finished = ran(source, "lambda_function.handler", None);
        let answered = returned(&finished, "the registered modules");
        let actual: Vec<String> = answered
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(::serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for name in &actual {
            assert!(
                ALLOWED_NATIVE_MODULES.contains(&name.as_str())
                    || SUPPLIED_MODULES.contains(&name.as_str()),
                "case {name}: a module outside the allow-list is reachable, got {actual:?}"
            );
        }
        for absent in [
            "_socket",
            "_ssl",
            "select",
            "_posixsubprocess",
            "mmap",
            "_ctypes",
            "_signal",
            "_interpreters",
            "fcntl",
            "resource",
            "pwd",
        ] {
            assert!(
                !actual.iter().any(|name| name == absent),
                "case {absent}: a host-reaching module is registered, got {actual:?}"
            );
        }
    }
}
