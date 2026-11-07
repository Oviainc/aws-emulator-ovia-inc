// SPDX-License-Identifier: CC0-1.0

//! `fs` and `fs/promises`: the filesystem a Lambda function has.
//!
//! A function's filesystem is not the host's and is not general: the deployment package is
//! mounted READ-ONLY at the task root, a layer's content read-only under the layer root,
//! and `/tmp` is the one writable directory. That is exactly what this module serves — the
//! read-only trees are the entries the store already holds for this version's package and
//! its layers (reached through the runtime's own module protocol, which is the only source
//! of a file there is), and `/tmp` is a tree held in this run's own memory, so a write
//! reaches nothing outside the engine and nothing of it survives the run.
//!
//! A write outside `/tmp` therefore fails the way it fails on the real runtime — `EROFS`,
//! read-only file system — and a path that names nothing fails with `ENOENT`. No host path
//! is opened here, and there is no host function in this module's reach that could open
//! one.

/// `fs`: the synchronous, callback and promise forms over the one in-memory tree.
pub(super) const FS_SOURCE: &str = r##"
'use strict';

var Buffer = require('buffer').Buffer;

// The one writable directory a function has. "Each Lambda function receives an ephemeral
// /tmp directory" — everything else a function can see is mounted read-only.
var WRITABLE_ROOT = '/tmp';

// How much `/tmp` may hold in this run. The configured ephemeral storage is the function's
// own; this is the ceiling on what one run keeps in the engine's memory for it, so a
// handler cannot turn a write loop into unbounded growth of this process.
var WRITABLE_BYTES = 512 * 1024 * 1024;

// The written tree: path -> { data, mtimeMs } for a file, path -> null for a directory.
var written = Object.create(null);
written[WRITABLE_ROOT] = null;
var writtenBytes = 0;

// The instant this run's read-only trees became visible, which is the only time there is
// for an entry that was never written.
var mountedAt = Date.now();

function host() {
    return globalThis.__lambdaRuntime;
}

function fault(code, description, syscall, path) {
    var error = new Error(code + ': ' + description + ', ' + syscall + " '" + path + "'");
    error.code = code;
    error.syscall = syscall;
    error.path = path;
    return error;
}

function normalize(path) {
    var text = typeof path === 'string' ? path : String(path);
    if (text.indexOf('file://') === 0) {
        text = text.slice('file://'.length);
    }
    var absolute = text.charAt(0) === '/';
    var segments = text.split('/');
    var out = [];
    for (var index = 0; index < segments.length; index += 1) {
        var segment = segments[index];
        if (segment === '' || segment === '.') {
            continue;
        }
        if (segment === '..') {
            out.pop();
            continue;
        }
        out.push(segment);
    }
    return (absolute ? '/' : '') + out.join('/');
}

function inWritableRoot(path) {
    return path === WRITABLE_ROOT || path.indexOf(WRITABLE_ROOT + '/') === 0;
}

function directoryOf(path) {
    var cut = path.lastIndexOf('/');
    return cut <= 0 ? '/' : path.slice(0, cut);
}

// Every path the read-only trees hold, which is every entry of the package and the layers.
function readOnlyPaths() {
    return host().filePaths();
}

function readOnlySource(path) {
    var source = host().fileSource(path);
    return source === null || source === undefined ? null : source;
}

function entryKind(path) {
    if (Object.prototype.hasOwnProperty.call(written, path)) {
        return written[path] === null ? 'directory' : 'file';
    }
    if (readOnlySource(path) !== null) {
        return 'file';
    }
    var prefix = path + '/';
    var paths = readOnlyPaths();
    for (var index = 0; index < paths.length; index += 1) {
        if (paths[index].indexOf(prefix) === 0) {
            return 'directory';
        }
    }
    var keys = Object.keys(written);
    for (var at = 0; at < keys.length; at += 1) {
        if (keys[at].indexOf(prefix) === 0) {
            return 'directory';
        }
    }
    return null;
}

function fileBytes(path, syscall) {
    if (Object.prototype.hasOwnProperty.call(written, path)) {
        if (written[path] === null) {
            throw fault('EISDIR', 'illegal operation on a directory', syscall, path);
        }
        return written[path].data;
    }
    var source = readOnlySource(path);
    if (source !== null) {
        return Buffer.from(source, 'utf8');
    }
    if (entryKind(path) === 'directory') {
        throw fault('EISDIR', 'illegal operation on a directory', syscall, path);
    }
    throw fault('ENOENT', 'no such file or directory', syscall, path);
}

function modifiedAt(path) {
    if (Object.prototype.hasOwnProperty.call(written, path) && written[path] !== null) {
        return written[path].mtimeMs;
    }
    return mountedAt;
}

// The one check that stands between a handler and a write: a path outside the writable
// root is on a read-only mount, so the write fails as it does on the real runtime.
function writable(path, syscall) {
    if (!inWritableRoot(path)) {
        throw fault('EROFS', 'read-only file system', syscall, path);
    }
}

function makeDirectories(path) {
    var segments = normalize(path).split('/');
    var walked = '';
    for (var index = 1; index < segments.length; index += 1) {
        walked += '/' + segments[index];
        if (!Object.prototype.hasOwnProperty.call(written, walked)) {
            written[walked] = null;
        }
    }
}

function store(path, bytes) {
    var held = Object.prototype.hasOwnProperty.call(written, path) && written[path] !== null
        ? written[path].data.length
        : 0;
    if (writtenBytes - held + bytes.length > WRITABLE_BYTES) {
        throw fault('ENOSPC', 'no space left on device', 'write', path);
    }
    writtenBytes = writtenBytes - held + bytes.length;
    written[path] = { data: bytes, mtimeMs: Date.now() };
}

function bytesOf(data, encoding) {
    if (typeof data === 'string') {
        return Buffer.from(data, encoding === undefined || encoding === null ? 'utf8' : encoding);
    }
    if (data === null || data === undefined) {
        return Buffer.alloc(0);
    }
    return Buffer.from(data);
}

function encodingOf(options) {
    if (typeof options === 'string') {
        return options;
    }
    if (options !== null && typeof options === 'object' && typeof options.encoding === 'string') {
        return options.encoding;
    }
    return null;
}

function Stats(path, kind) {
    var isFile = kind === 'file';
    this.size = isFile ? fileBytes(path, 'stat').length : 0;
    // The mode a Lambda filesystem reports: a writable file under the ephemeral directory,
    // and a read-only one everywhere else.
    this.mode = (isFile ? 0o100000 : 0o040000) | (inWritableRoot(path) ? 0o755 : 0o555);
    this.mtimeMs = modifiedAt(path);
    this.mtime = new Date(this.mtimeMs);
    this.ctimeMs = this.mtimeMs;
    this.ctime = this.mtime;
    this.atimeMs = this.mtimeMs;
    this.atime = this.mtime;
    this.birthtimeMs = this.mtimeMs;
    this.birthtime = this.mtime;
    this.uid = 993;
    this.gid = 990;
    this.nlink = 1;
    this.blksize = 4096;
    this.blocks = Math.ceil(this.size / 512);
    this.kind = kind;
}

Stats.prototype.isFile = function () { return this.kind === 'file'; };
Stats.prototype.isDirectory = function () { return this.kind === 'directory'; };
Stats.prototype.isSymbolicLink = function () { return false; };
Stats.prototype.isBlockDevice = function () { return false; };
Stats.prototype.isCharacterDevice = function () { return false; };
Stats.prototype.isFIFO = function () { return false; };
Stats.prototype.isSocket = function () { return false; };

function readFileSync(path, options) {
    var resolved = normalize(path);
    var bytes = fileBytes(resolved, 'open');
    var encoding = encodingOf(options);
    return encoding === null ? Buffer.from(bytes) : bytes.toString(encoding);
}

function writeFileSync(path, data, options) {
    var resolved = normalize(path);
    writable(resolved, 'open');
    makeDirectories(directoryOf(resolved));
    store(resolved, bytesOf(data, encodingOf(options)));
}

function appendFileSync(path, data, options) {
    var resolved = normalize(path);
    writable(resolved, 'open');
    var held = Object.prototype.hasOwnProperty.call(written, resolved) && written[resolved] !== null
        ? written[resolved].data
        : Buffer.alloc(0);
    makeDirectories(directoryOf(resolved));
    store(resolved, Buffer.concat([held, bytesOf(data, encodingOf(options))]));
}

function existsSync(path) {
    return entryKind(normalize(path)) !== null;
}

function statSync(path) {
    var resolved = normalize(path);
    var kind = entryKind(resolved);
    if (kind === null) {
        throw fault('ENOENT', 'no such file or directory', 'stat', resolved);
    }
    return new Stats(resolved, kind);
}

function readdirSync(path, options) {
    var resolved = normalize(path);
    var kind = entryKind(resolved);
    if (kind === null) {
        throw fault('ENOENT', 'no such file or directory', 'scandir', resolved);
    }
    if (kind !== 'directory') {
        throw fault('ENOTDIR', 'not a directory', 'scandir', resolved);
    }
    var prefix = resolved === '/' ? '/' : resolved + '/';
    var names = [];
    var seen = Object.create(null);
    var add = function (candidate) {
        if (candidate.indexOf(prefix) !== 0) {
            return;
        }
        var tail = candidate.slice(prefix.length);
        if (tail === '') {
            return;
        }
        var cut = tail.indexOf('/');
        var name = cut < 0 ? tail : tail.slice(0, cut);
        if (!seen[name]) {
            seen[name] = true;
            names.push(name);
        }
    };
    var paths = readOnlyPaths();
    for (var index = 0; index < paths.length; index += 1) {
        add(paths[index]);
    }
    var keys = Object.keys(written);
    for (var at = 0; at < keys.length; at += 1) {
        add(keys[at]);
    }
    names.sort();
    if (options !== null && typeof options === 'object' && options.withFileTypes === true) {
        return names.map(function (name) {
            var kindOf = entryKind(prefix + name);
            return {
                name: name,
                parentPath: resolved,
                path: resolved,
                isFile: function () { return kindOf === 'file'; },
                isDirectory: function () { return kindOf === 'directory'; },
                isSymbolicLink: function () { return false; }
            };
        });
    }
    return names;
}

function mkdirSync(path, options) {
    var resolved = normalize(path);
    writable(resolved, 'mkdir');
    var recursive = options !== null && typeof options === 'object' && options.recursive === true;
    if (!recursive && entryKind(resolved) !== null) {
        throw fault('EEXIST', 'file already exists', 'mkdir', resolved);
    }
    if (!recursive && entryKind(directoryOf(resolved)) === null) {
        throw fault('ENOENT', 'no such file or directory', 'mkdir', resolved);
    }
    makeDirectories(resolved);
    return recursive ? resolved : undefined;
}

function unlinkSync(path) {
    var resolved = normalize(path);
    writable(resolved, 'unlink');
    if (!Object.prototype.hasOwnProperty.call(written, resolved) || written[resolved] === null) {
        throw fault('ENOENT', 'no such file or directory', 'unlink', resolved);
    }
    writtenBytes -= written[resolved].data.length;
    delete written[resolved];
}

function rmSync(path, options) {
    var resolved = normalize(path);
    writable(resolved, 'unlink');
    var recursive = options !== null && typeof options === 'object' && options.recursive === true;
    var force = options !== null && typeof options === 'object' && options.force === true;
    var kind = entryKind(resolved);
    if (kind === null) {
        if (force) {
            return;
        }
        throw fault('ENOENT', 'no such file or directory', 'unlink', resolved);
    }
    if (kind === 'file') {
        unlinkSync(resolved);
        return;
    }
    if (!recursive) {
        throw fault('ERR_FS_EISDIR', 'illegal operation on a directory', 'rm', resolved);
    }
    var keys = Object.keys(written);
    for (var index = 0; index < keys.length; index += 1) {
        if (keys[index] === resolved || keys[index].indexOf(resolved + '/') === 0) {
            if (written[keys[index]] !== null) {
                writtenBytes -= written[keys[index]].data.length;
            }
            delete written[keys[index]];
        }
    }
}

function rmdirSync(path, options) {
    var resolved = normalize(path);
    writable(resolved, 'rmdir');
    if (entryKind(resolved) !== 'directory') {
        throw fault('ENOENT', 'no such file or directory', 'rmdir', resolved);
    }
    rmSync(resolved, { recursive: true, force: true });
}

function renameSync(from, to) {
    var source = normalize(from);
    var target = normalize(to);
    writable(target, 'rename');
    var bytes = fileBytes(source, 'rename');
    writeFileSync(target, bytes);
    if (inWritableRoot(source)) {
        unlinkSync(source);
    }
}

function copyFileSync(from, to) {
    writeFileSync(normalize(to), fileBytes(normalize(from), 'copyfile'));
}

function accessSync(path, mode) {
    var resolved = normalize(path);
    if (entryKind(resolved) === null) {
        throw fault('ENOENT', 'no such file or directory', 'access', resolved);
    }
    var wanted = mode === undefined ? 0 : Number(mode);
    if ((wanted & 2) === 2 && !inWritableRoot(resolved)) {
        throw fault('EROFS', 'read-only file system', 'access', resolved);
    }
}

function realpathSync(path) {
    var resolved = normalize(path);
    if (entryKind(resolved) === null) {
        throw fault('ENOENT', 'no such file or directory', 'lstat', resolved);
    }
    return resolved;
}

// Every synchronous operation above, as the one table the callback and promise forms are
// derived from: the behaviour is written once and the three shapes Node offers it under are
// the same call.
var OPERATIONS = {
    readFile: readFileSync,
    writeFile: writeFileSync,
    appendFile: appendFileSync,
    stat: statSync,
    lstat: statSync,
    readdir: readdirSync,
    mkdir: mkdirSync,
    unlink: unlinkSync,
    rm: rmSync,
    rmdir: rmdirSync,
    rename: renameSync,
    copyFile: copyFileSync,
    access: accessSync,
    realpath: realpathSync
};

function callbackForm(run) {
    return function () {
        var args = Array.prototype.slice.call(arguments);
        var done = typeof args[args.length - 1] === 'function' ? args.pop() : null;
        var answered;
        var raised = null;
        try {
            answered = run.apply(null, args);
        } catch (error) {
            raised = error;
        }
        if (done === null) {
            if (raised !== null) {
                throw raised;
            }
            return;
        }
        Promise.resolve().then(function () {
            if (raised !== null) {
                done(raised);
                return;
            }
            done(null, answered);
        });
    };
}

function promiseForm(run) {
    return function () {
        var args = Array.prototype.slice.call(arguments);
        try {
            return Promise.resolve(run.apply(null, args));
        } catch (error) {
            return Promise.reject(error);
        }
    };
}

exports.constants = { F_OK: 0, X_OK: 1, W_OK: 2, R_OK: 4 };
exports.Stats = Stats;
exports.readFileSync = readFileSync;
exports.writeFileSync = writeFileSync;
exports.appendFileSync = appendFileSync;
exports.existsSync = existsSync;
exports.statSync = statSync;
exports.lstatSync = statSync;
exports.readdirSync = readdirSync;
exports.mkdirSync = mkdirSync;
exports.mkdtempSync = function (prefix) {
    var made = normalize(prefix + require('crypto').randomBytes(3).toString('hex'));
    mkdirSync(made, { recursive: true });
    return made;
};
exports.unlinkSync = unlinkSync;
exports.rmSync = rmSync;
exports.rmdirSync = rmdirSync;
exports.renameSync = renameSync;
exports.copyFileSync = copyFileSync;
exports.accessSync = accessSync;
exports.realpathSync = realpathSync;
exports.chmodSync = function (path) { writable(normalize(path), 'chmod'); };
exports.utimesSync = function (path) { writable(normalize(path), 'utimes'); };

var promises = {};
var names = Object.keys(OPERATIONS);
for (var index = 0; index < names.length; index += 1) {
    exports[names[index]] = callbackForm(OPERATIONS[names[index]]);
    promises[names[index]] = promiseForm(OPERATIONS[names[index]]);
}
promises.mkdtemp = promiseForm(exports.mkdtempSync);
promises.constants = exports.constants;
exports.promises = promises;
"##;

/// `fs/promises`: the promise form of the same tree, as its own module.
pub(super) const FS_PROMISES_SOURCE: &str = r##"
'use strict';

module.exports = require('fs').promises;
"##;
