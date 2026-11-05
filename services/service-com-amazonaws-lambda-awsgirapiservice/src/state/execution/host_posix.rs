// SPDX-License-Identifier: CC0-1.0

//! The `posix` module a Lambda function's Python runs on: this emulator's own, with no
//! host call in it at all.
//!
//! Python's `os` is built on a module named `posix`, and the import system itself requires
//! one to exist ("importlib requires posix or nt"), so an interpreter has to have one. The
//! engine's own would be the HOST's — its calls open host files, start host processes and
//! read the host environment — and it is not registered at all (it needs an engine feature
//! this crate does not build, see `Cargo.toml`). This is the one that is: a module whose
//! whole surface is the handful of calls below, every one of which is answered from this
//! process's own memory.
//!
//! * `environ` is an empty mapping the invocation's own variables are put into (the
//!   runtime source does that), so `os.environ` is the function's environment and holds no
//!   host variable at all;
//! * `putenv` / `unsetenv` change that mapping only — writing an environment variable from
//!   guest code cannot reach this process's environment;
//! * `getcwd` / `getcwdb` answer the task root the runtime reports, which is the directory
//!   a Lambda function's code is at; there is no host directory to be in;
//! * `urandom` answers bytes from the host's own generator, which is where a Python
//!   program's randomness comes from; no host device is opened for it;
//! * `getpid` answers the single process id a function sees.
//!
//! Every other name a Python program may look for on `posix` is simply ABSENT, so the call
//! raises `AttributeError` where the guest reaches for it: there is no file to open, no
//! directory to list, no process to start and no descriptor to read. `os` builds itself on
//! what is here and guards everything else (`_exists(...)`), which is why a missing name is
//! the right answer rather than a stub that pretends.

/// The module, as the engine's own module macro declares one. Registered under the name
/// `posix`, which is the name Python's `os` and the import system look for.
#[::rustpython_vm::pymodule(name = "posix")]
pub(crate) mod module {
    use ::rustpython_vm::builtins::PyDictRef;
    use ::rustpython_vm::{PyObjectRef, PyResult, VirtualMachine};

    /// The task root a Lambda function's code is at, which is the only directory a run is
    /// ever in.
    const TASK_ROOT: &str = "/var/task";

    /// The ENVIRONMENT of the invocation, as an in-memory mapping. Empty until the runtime
    /// source puts the invocation's own variables into it; no host environment variable is
    /// ever in it.
    #[pyattr]
    fn environ(vm: &VirtualMachine) -> PyDictRef {
        vm.ctx.new_dict()
    }

    /// The directory a run is in: the task root, always.
    #[pyfunction]
    fn getcwd(vm: &VirtualMachine) -> PyObjectRef {
        vm.ctx.new_str(TASK_ROOT).into()
    }

    /// [`getcwd`] as bytes, which is the other form `os` asks for.
    #[pyfunction]
    fn getcwdb(vm: &VirtualMachine) -> PyObjectRef {
        vm.ctx.new_bytes(TASK_ROOT.as_bytes().to_vec()).into()
    }

    /// The path `path` names, as `os.path` asks for it: a string or bytes is itself, and
    /// anything else is asked for its own `__fspath__`. Pure conversion — it touches no
    /// filesystem, which is why it is answered rather than refused.
    #[pyfunction]
    fn fspath(path: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        if path
            .downcast_ref::<::rustpython_vm::builtins::PyStr>()
            .is_some()
            || path
                .downcast_ref::<::rustpython_vm::builtins::PyBytes>()
                .is_some()
        {
            return Ok(path);
        }
        let named = path.get_attr("__fspath__", vm)?;
        named.call((), vm)
    }

    /// The process id a run sees. One run is one invocation, so it is a constant.
    #[pyfunction]
    fn getpid() -> i32 {
        1
    }

    /// `wanted` random bytes, from the host's own generator. No host device is opened: the
    /// bytes come from the same generator this emulator mints identifiers with.
    #[pyfunction]
    fn urandom(wanted: usize, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let wanted = wanted.min(MAX_RANDOM_BYTES);
        let mut bytes = Vec::with_capacity(wanted);
        while bytes.len() < wanted {
            bytes.extend_from_slice(::uuid::Uuid::new_v4().as_bytes());
        }
        bytes.truncate(wanted);
        Ok(vm.ctx.new_bytes(bytes).into())
    }

    /// The most bytes one `urandom` call answers, so a one-line handler cannot ask for an
    /// unbounded buffer.
    const MAX_RANDOM_BYTES: usize = 1024 * 1024;

    /// Set one variable of the invocation's environment. It reaches the mapping above and
    /// nothing else: this process's own environment is not touched.
    #[pyfunction]
    fn putenv(key: PyObjectRef, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
        let environ = environ_of(vm)?;
        environ.set_item(&*key, value, vm)
    }

    /// Remove one variable of the invocation's environment, in the same mapping.
    #[pyfunction]
    fn unsetenv(key: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
        let environ = environ_of(vm)?;
        let _ = environ.del_item(&*key, vm);
        Ok(())
    }

    /// The live `environ` mapping of this interpreter's `posix` module.
    fn environ_of(vm: &VirtualMachine) -> PyResult<PyDictRef> {
        let module = vm.import("posix", 0)?;
        let environ = module.get_attr("environ", vm)?;
        environ
            .downcast::<::rustpython_vm::builtins::PyDict>()
            .map_err(|_| vm.new_runtime_error("the environment mapping is not a mapping"))
    }
}
