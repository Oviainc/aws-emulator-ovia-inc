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
//! Every other CALL a Python program may look for on `posix` is simply ABSENT, so it raises
//! where the guest reaches for it: there is no file to open, no directory to list, no
//! process to start and no descriptor to read. `os` builds itself on what is here and
//! guards everything else (`_exists(...)`), which is why a missing call is the right answer
//! rather than a stub that pretends.
//!
//! What `os` re-exports as DATA is a different thing, and it is all here
//! ([`install_data_surface`]): the `stat_result` type and the `open` flags are a type and a
//! handful of numbers, not calls, and the standard library reads them off `os` WHILE BEING
//! IMPORTED — `shutil` (and so `tempfile`, `zipfile` and `tarfile`) reads `os.stat_result`,
//! `glob` (and so `pathlib`) reads `os.O_RDONLY` — so a module missing them is one those
//! modules cannot be imported from at all, which is not what a managed runtime's complete
//! standard library does. Holding them reaches nothing: there is no call here to pass a
//! flag to, and the only `stat_result` a run ever sees is one guest code built itself.

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

    /// Register callables to be run around a FORK of this process.
    ///
    /// Nothing is ever forked here — `fork` raises, and no call in this module starts a
    /// process — so a registration is accepted and the callables it names are never run,
    /// which is exactly what happens to a hook registered in a process that never forks.
    /// It is answered rather than refused because the standard library registers one while
    /// being imported: `random` re-seeds itself in a forked child, and `secrets`,
    /// `statistics` and everything else that imports `random` would otherwise fail to
    /// import at all.
    ///
    /// The arguments are checked the way CPython checks them: the three hook points are
    /// keyword-only, at least one has to be given, and each has to be callable.
    #[pyfunction]
    fn register_at_fork(args: ::rustpython_vm::function::FuncArgs, vm: &VirtualMachine) -> PyResult<()> {
        if !args.args.is_empty() {
            return Err(vm.new_type_error("register_at_fork() takes no positional arguments"));
        }
        if args.kwargs.is_empty() {
            return Err(vm.new_type_error(
                "Invalid arguments: at least one argument must be provided",
            ));
        }
        for (name, hook) in &args.kwargs {
            let Ok(name) = name.as_str() else {
                return Err(vm.new_type_error("keywords must be strings"));
            };
            if !matches!(name, "before" | "after_in_child" | "after_in_parent") {
                return Err(vm.new_type_error(format!(
                    "register_at_fork() got an unexpected keyword argument '{name}'"
                )));
            }
            if !hook.is_callable() {
                return Err(
                    vm.new_type_error(format!("Invalid arguments: {name} must be callable"))
                );
            }
        }
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

/// The flag NUMBERS a POSIX module carries, as the platform's own C headers define them —
/// read from `libc` rather than written out here, so what a run sees is this machine's.
///
/// They are DATA: the module above has no `open`, no `access` and no `lseek` to pass one
/// to. The standard library reads them off `os` while being imported (`glob` builds its
/// directory-open flags out of `os.O_RDONLY`, `shutil` and the modules that import it read
/// `os.F_OK`), which is why they are here at all.
const POSIX_FLAGS: &[(&str, i32)] = &[
    // The modes an access check names.
    ("F_OK", ::libc::F_OK),
    ("R_OK", ::libc::R_OK),
    ("W_OK", ::libc::W_OK),
    ("X_OK", ::libc::X_OK),
    // The flags an open takes.
    ("O_RDONLY", ::libc::O_RDONLY),
    ("O_WRONLY", ::libc::O_WRONLY),
    ("O_RDWR", ::libc::O_RDWR),
    ("O_APPEND", ::libc::O_APPEND),
    ("O_CREAT", ::libc::O_CREAT),
    ("O_EXCL", ::libc::O_EXCL),
    ("O_TRUNC", ::libc::O_TRUNC),
    ("O_NONBLOCK", ::libc::O_NONBLOCK),
    ("O_NOCTTY", ::libc::O_NOCTTY),
    ("O_CLOEXEC", ::libc::O_CLOEXEC),
    ("O_DIRECTORY", ::libc::O_DIRECTORY),
    ("O_NOFOLLOW", ::libc::O_NOFOLLOW),
    ("O_SYNC", ::libc::O_SYNC),
    ("O_DSYNC", ::libc::O_DSYNC),
    // Where a seek counts from.
    ("SEEK_SET", ::libc::SEEK_SET),
    ("SEEK_CUR", ::libc::SEEK_CUR),
    ("SEEK_END", ::libc::SEEK_END),
];

/// The TYPE a POSIX `stat` answers with, which `os` re-exports as `os.stat_result`.
///
/// It is the sequence of a `stat` structure's fields, in the order CPython's own
/// `stat_result` carries them, with each field also reachable by its name — so code that
/// unpacks one, indexes it or reads `st_size` off it reads the field it names. Written as
/// the type it is rather than as a stand-in: a run that builds one gets a value that
/// behaves like the standard library's, and nothing here reads a file to fill one in.
const STAT_RESULT_SOURCE: &str = r#"
class stat_result(tuple):
    """The fields a POSIX stat answers for one file."""

    __module__ = "os"

    _fields = (
        "st_mode",
        "st_ino",
        "st_dev",
        "st_nlink",
        "st_uid",
        "st_gid",
        "st_size",
        "st_atime",
        "st_mtime",
        "st_ctime",
    )
    n_fields = 10
    n_sequence_fields = 10
    n_unnamed_fields = 0

    def __new__(cls, sequence, named=None):
        values = tuple(sequence)
        if len(values) < cls.n_sequence_fields:
            raise TypeError(
                "stat_result() takes an at least %d-sequence (%d-sequence given)"
                % (cls.n_sequence_fields, len(values))
            )
        built = tuple.__new__(cls, values[: cls.n_sequence_fields])
        # The fields a stat structure carries beyond the sequence (the nanosecond times,
        # the block counts) travel in a mapping, which is how CPython's own constructor
        # takes them, and each becomes an attribute of its own.
        for name, value in dict(named or {}).items():
            setattr(built, name, value)
        return built

    def __repr__(self):
        return "os.stat_result(%s)" % ", ".join(
            "%s=%r" % (name, value) for name, value in zip(self._fields, self)
        )


def _bind_fields():
    for index, name in enumerate(stat_result._fields):
        setattr(stat_result, name, property(lambda self, index=index: self[index]))


_bind_fields()
"#;

/// Install on the `posix` module above everything `os` re-exports from it as DATA: the
/// [`STAT_RESULT_SOURCE`] type and the [`POSIX_FLAGS`].
///
/// Called before `os` is imported for the first time, because `os` builds itself out of the
/// names it finds on this module (`from posix import *`). Nothing here is containment: the
/// surface a run can reach is decided by which native modules are registered and which
/// calls the module above has, neither of which this changes.
pub(crate) fn install_data_surface(
    vm: &::rustpython_vm::VirtualMachine,
) -> ::rustpython_vm::PyResult<()> {
    let system = vm.import("posix", 0)?;
    for (name, value) in POSIX_FLAGS {
        system.set_attr(*name, vm.ctx.new_int(*value), vm)?;
    }
    let scope = vm.new_scope_with_builtins();
    let code = vm
        .compile(
            STAT_RESULT_SOURCE,
            ::rustpython_vm::compiler::Mode::Exec,
            "<lambda-posix>".to_owned(),
        )
        .map_err(|error| {
            vm.new_runtime_error(format!("the stat_result source does not compile: {error}"))
        })?;
    vm.run_code_obj(code, scope.clone())?;
    let stat_result = scope.globals.get_item("stat_result", vm)?;
    system.set_attr("stat_result", stat_result, vm)?;
    Ok(())
}

