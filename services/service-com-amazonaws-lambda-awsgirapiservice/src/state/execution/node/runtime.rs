// SPDX-License-Identifier: CC0-1.0

//! The RUNTIME a JavaScript handler runs under, in the language the handler is written in.
//!
//! A node runtime's job is small and entirely defined by what the handler sees: resolve the
//! handler's module out of the deployment package, build the context object, call the
//! handler with the event and the context, and report back either what it returned or the
//! error document Lambda answers for an exception. That is this source: it runs INSIDE the
//! engine [`super`] builds, because every one of those steps is a JavaScript one — the
//! module system is JavaScript's, `JSON` reads the event and writes the response, and an
//! `Error`'s own `stack` is the trace.
//!
//! What it is NOT is the containment or the bounds. The whole host surface of a run is the
//! three functions [`super::install_host`] installs — the invocation log, a package entry's
//! source, and a core module's source — and this source takes all three off the global
//! object once it holds them. The deadline, the step ceiling and the memory ceiling are the
//! engine's and the host's ([`super::Bounds`]), so no rebinding or deletion of a name here
//! can lift one.
//!
//! The protocol between host and runtime is one frozen global, `__lambdaRuntime`, so the
//! host's own engine surface stays as small as a string in and a string out:
//!
//! * `start(input)` — begins the invocation the JSON document `input` describes: the
//!   handler name, the event, the environment the invocation runs with, and the deadline.
//! * `poll()` — the JSON document of what the invocation came to, once it has, and `null`
//!   while it has not.
//! * `waiting()` — what an unsettled invocation is waiting for: the promise an
//!   asynchronous handler returned, or the empty event loop a non-async handler's answer is
//!   reported at.
//! * `nextTimerDelayMs()` / `runDueTimers()` — the timer queue, which the host drives: it
//!   is the host that holds the clock a run is bounded by, so it is the host that decides
//!   when a timer is due.
//! * `eventLoopEmpty()` — told by the host when no job and no timer is left to run.
//! * `resolveModule(base, name)` / `moduleSource(id)` — how an `import` is resolved and
//!   loaded, which is the same resolution `require` performs: a file of this package, or a
//!   core module this emulator supplies, and nothing else.

/// The JavaScript source of the runtime, evaluated once per invocation in a fresh engine.
pub(super) const RUNTIME_SOURCE: &str = r##"
'use strict';
(function () {
    var TASK_ROOT = '/var/task';

    // The three host functions, taken off the global object as soon as they are held: from
    // here on the only way to reach one is through this closure.
    var hostLog = globalThis.__hostLog;
    var hostModuleSource = globalThis.__hostModuleSource;
    var hostCoreSource = globalThis.__hostCoreSource;
    delete globalThis.__hostLog;
    delete globalThis.__hostModuleSource;
    delete globalThis.__hostCoreSource;

    // Dynamic import is the one way a module of the package is loaded as an ES module. It
    // is compiled here, once, so a run never depends on it being available at any later
    // point.
    var dynamicImport = null;
    try {
        dynamicImport = new Function('specifier', 'return import(specifier);');
    } catch (error) {
        dynamicImport = null;
    }

    var state = null;
    var cache = Object.create(null);
    var timers = [];
    var nextTimerId = 1;

    // ---------------------------------------------------------------- paths

    // The two sources a module is read from, normalised: a host function that holds no
    // such entry answers with nothing, and `null` is what that is here. A path that is not
    // one (a resolution that refused the specifier) is never looked up at all.
    function sourceOf(path) {
        if (typeof path !== 'string' || path === '') {
            return null;
        }
        var source = hostModuleSource(path);
        return source === null || source === undefined ? null : source;
    }

    function coreSourceOf(name) {
        var source = hostCoreSource(name);
        return source === null || source === undefined ? null : source;
    }

    function endsWith(text, suffix) {
        return text.length >= suffix.length
            && text.slice(text.length - suffix.length) === suffix;
    }

    function directoryOf(path) {
        var cut = path.lastIndexOf('/');
        return cut < 0 ? '' : path.slice(0, cut);
    }

    // The one way a specifier becomes a path INSIDE the package. The segments of
    // `directory` and `request` are collapsed FIRST — `.` dropped, `..` applied — and a
    // path that would leave the package, or that names no file at all, is refused rather
    // than clamped: no specifier can therefore name anything but an entry of this
    // package, whatever `..` or absolute form it is written in. Every resolution below
    // goes through this, so there is no second route to a lookup key.
    function packagePath(directory, request) {
        if (typeof request !== 'string') {
            return null;
        }
        var segments = (directory === '' ? [] : directory.split('/')).concat(request.split('/'));
        var out = [];
        for (var index = 0; index < segments.length; index += 1) {
            var segment = segments[index];
            if (segment === '' || segment === '.') {
                continue;
            }
            if (segment === '..') {
                if (out.length === 0) {
                    return null;
                }
                out.pop();
                continue;
            }
            out.push(segment);
        }
        return out.length === 0 ? null : out.join('/');
    }

    // ---------------------------------------------------------------- module resolution

    function coreName(request) {
        var bare = request.indexOf('node:') === 0 ? request.slice(5) : request;
        return coreSourceOf(bare) === null ? null : bare;
    }

    // How many `package.json` "main" entries one resolution may follow. A manifest whose
    // `main` leads back to a directory already being resolved would otherwise make the
    // resolution of a package the caller wrote recurse without end, so the chain is
    // bounded here and a package past the bound resolves to nothing at all.
    var MAX_MANIFEST_HOPS = 8;

    // The package entry `base` names, trying the suffixes a Node resolution tries and
    // then the directory forms. `null` when the package holds no such file.
    function fileCandidate(base) {
        return candidateOf(base, 0);
    }

    function candidateOf(base, hops) {
        var suffixes = ['', '.js', '.json', '.cjs', '.mjs'];
        var index = 0;
        for (index = 0; index < suffixes.length; index += 1) {
            if (sourceOf(base + suffixes[index]) !== null) {
                return base + suffixes[index];
            }
        }
        var manifest = hops < MAX_MANIFEST_HOPS ? sourceOf(base + '/package.json') : null;
        if (manifest !== null) {
            var main = null;
            try {
                main = JSON.parse(manifest).main;
            } catch (error) {
                main = null;
            }
            if (typeof main === 'string' && main !== '') {
                var joined = packagePath(base, main);
                var resolved = joined === null ? null : candidateOf(joined, hops + 1);
                if (resolved !== null) {
                    return resolved;
                }
            }
        }
        var indexes = ['/index.js', '/index.json', '/index.cjs', '/index.mjs'];
        for (index = 0; index < indexes.length; index += 1) {
            if (sourceOf(base + indexes[index]) !== null) {
                return base + indexes[index];
            }
        }
        return null;
    }

    function packageCandidate(directory, request) {
        var current = directory;
        for (;;) {
            var base = packagePath(current, 'node_modules/' + request);
            var found = base === null ? null : fileCandidate(base);
            if (found !== null) {
                return found;
            }
            if (current === '') {
                return null;
            }
            current = directoryOf(current);
        }
    }

    // Resolve `request` as `require` does, from the directory `directory`: a core module
    // this emulator supplies, a file of this package, or nothing. A path of the HOST
    // resolves to nothing, because the only files there are are the package's — every
    // branch below answers with a [`packagePath`], which is inside the package or null.
    function resolveRequest(directory, request) {
        if (typeof request !== 'string' || request === '') {
            return null;
        }
        var core = coreName(request);
        if (core !== null) {
            return 'core:' + core;
        }
        if (request.charAt(0) === '.') {
            var relative = packagePath(directory, request);
            return relative === null ? null : fileCandidate(relative);
        }
        if (request.charAt(0) === '/') {
            // An absolute specifier is a path of the task root or it is nothing, and the
            // tail is normalised BEFORE it is looked up, so a `..` in it cannot reach
            // past the root the prefix just established.
            if (request.indexOf(TASK_ROOT + '/') !== 0) {
                return null;
            }
            var absolute = packagePath('', request.slice(TASK_ROOT.length + 1));
            return absolute === null ? null : fileCandidate(absolute);
        }
        return packageCandidate(directory, request);
    }

    function moduleNotFound(request) {
        var error = new Error("Cannot find module '" + request + "'");
        error.code = 'MODULE_NOT_FOUND';
        return error;
    }

    // ---------------------------------------------------------------- module loading

    function compilesAsCommonJs(source) {
        try {
            new Function('exports', 'require', 'module', '__filename', '__dirname', source + '\n');
            return true;
        } catch (error) {
            return false;
        }
    }

    // Whether the nearest `package.json` above `path` declares its files to be ES
    // modules, which is what `"type": "module"` says.
    function declaredEsModule(path) {
        var directory = directoryOf(path);
        for (;;) {
            var manifest = sourceOf(packagePath(directory, 'package.json'));
            if (manifest !== null) {
                try {
                    return JSON.parse(manifest).type === 'module';
                } catch (error) {
                    return false;
                }
            }
            if (directory === '') {
                return false;
            }
            directory = directoryOf(directory);
        }
    }

    function requireFrom(directory) {
        var required = function (request) {
            var id = resolveRequest(directory, request);
            if (id === null) {
                throw moduleNotFound(request);
            }
            return requireId(id);
        };
        required.resolve = function (request) {
            var id = resolveRequest(directory, request);
            if (id === null) {
                throw moduleNotFound(request);
            }
            return id.indexOf('core:') === 0 ? id.slice(5) : TASK_ROOT + '/' + id;
        };
        required.cache = cache;
        return required;
    }

    function requireId(id) {
        var held = cache[id];
        if (held !== undefined) {
            return held.exports;
        }
        var core = id.indexOf('core:') === 0;
        var source = core ? coreSourceOf(id.slice(5)) : sourceOf(id);
        if (source === null) {
            throw moduleNotFound(core ? id.slice(5) : id);
        }
        var filename = core ? 'node:' + id.slice(5) : TASK_ROOT + '/' + id;
        var module = {
            id: filename,
            filename: filename,
            exports: {},
            loaded: false,
            children: [],
            paths: []
        };
        cache[id] = module;
        try {
            if (!core && endsWith(id, '.json')) {
                module.exports = JSON.parse(source);
            } else {
                var directory = core ? '' : directoryOf(id);
                var compiled = new Function(
                    'exports',
                    'require',
                    'module',
                    '__filename',
                    '__dirname',
                    source + '\n'
                );
                compiled.call(
                    module.exports,
                    module.exports,
                    requireFrom(directory),
                    module,
                    filename,
                    core ? 'node:' + id.slice(5) : TASK_ROOT + (directory === '' ? '' : '/' + directory)
                );
            }
            module.loaded = true;
        } catch (error) {
            delete cache[id];
            throw error;
        }
        return module.exports;
    }

    // The source the host declares an ES module from, for an id this runtime resolved.
    ///
    // A file that is ES module source is declared as it was written. A file that is
    // CommonJS — and a core module — is declared as a module whose default export is what
    // `require` of it produces, which is how a CommonJS module is imported.
    function moduleSource(id) {
        if (id.indexOf('core:') === 0) {
            return coreWrapper(id);
        }
        var source = sourceOf(id);
        if (source === null) {
            return null;
        }
        if (endsWith(id, '.json')) {
            return 'export default ' + source + ';\n';
        }
        if (endsWith(id, '.mjs')) {
            return source;
        }
        if (!endsWith(id, '.cjs') && (declaredEsModule(id) || !compilesAsCommonJs(source))) {
            return source;
        }
        return 'const __exports = globalThis.__lambdaRuntime.requireId('
            + JSON.stringify(id) + ');\nexport default __exports;\n';
    }

    function isIdentifier(name) {
        return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) && name !== 'default';
    }

    // A core module as an ES module: its own object as the default export, and one named
    // export per member it carries, which is what an `import { member } from` needs.
    function coreWrapper(id) {
        var exported = requireId(id);
        var lines = [
            'const __core = globalThis.__lambdaRuntime.requireId(' + JSON.stringify(id) + ');',
            'export default __core;'
        ];
        var keys = Object.keys(exported === null ? {} : exported);
        for (var index = 0; index < keys.length; index += 1) {
            if (isIdentifier(keys[index])) {
                lines.push('export const ' + keys[index] + ' = __core.' + keys[index] + ';');
            }
        }
        return lines.join('\n') + '\n';
    }

    // Resolve an `import` specifier, through the SAME resolution `require` performs, so an
    // import reaches a file of this package or a core module and nothing else. The base is
    // the importing module's own id when the package holds it, and the task root otherwise
    // — which is where the runtime's own import of the handler's module starts from.
    function resolveModule(base, name) {
        var core = coreName(name);
        if (core !== null) {
            return 'core:' + core;
        }
        var directory = sourceOf(base) === null ? '' : directoryOf(base);
        return resolveRequest(directory, name);
    }

    // ---------------------------------------------------------------- the console

    var util = requireId('core:util');

    function writeLine(args) {
        hostLog(util.format.apply(null, Array.prototype.slice.call(args)) + '\n');
    }

    var console = {
        log: function () { writeLine(arguments); },
        info: function () { writeLine(arguments); },
        debug: function () { writeLine(arguments); },
        dir: function () { writeLine(arguments); },
        warn: function () { writeLine(arguments); },
        error: function () { writeLine(arguments); },
        trace: function () { writeLine(arguments); }
    };
    globalThis.console = console;

    // ---------------------------------------------------------------- timers

    function timerHandle(timer) {
        return {
            id: timer.id,
            ref: function () { return this; },
            unref: function () { return this; },
            hasRef: function () { return true; }
        };
    }

    function addTimer(callback, delay, args, repeating) {
        if (typeof callback !== 'function') {
            throw new TypeError('The "callback" argument must be of type function.');
        }
        var milliseconds = Number(delay);
        if (!isFinite(milliseconds) || milliseconds < 0) {
            milliseconds = 0;
        }
        var timer = {
            id: nextTimerId,
            due: Date.now() + milliseconds,
            callback: callback,
            args: args,
            every: repeating ? Math.max(1, milliseconds) : null
        };
        nextTimerId += 1;
        timers.push(timer);
        return timerHandle(timer);
    }

    function clearTimer(handle) {
        var id = handle === null || typeof handle !== 'object' ? handle : handle.id;
        for (var index = 0; index < timers.length; index += 1) {
            if (timers[index].id === id) {
                timers.splice(index, 1);
                return;
            }
        }
    }

    function nextTimerDelayMs() {
        if (timers.length === 0) {
            return null;
        }
        var earliest = timers[0].due;
        for (var index = 1; index < timers.length; index += 1) {
            if (timers[index].due < earliest) {
                earliest = timers[index].due;
            }
        }
        return Math.max(0, earliest - Date.now());
    }

    function runDueTimers() {
        var now = Date.now();
        var due = [];
        var remaining = [];
        for (var index = 0; index < timers.length; index += 1) {
            if (timers[index].due <= now) {
                due.push(timers[index]);
                if (timers[index].every !== null) {
                    timers[index].due = now + timers[index].every;
                    remaining.push(timers[index]);
                }
            } else {
                remaining.push(timers[index]);
            }
        }
        timers = remaining;
        for (var ran = 0; ran < due.length; ran += 1) {
            try {
                due[ran].callback.apply(undefined, due[ran].args);
            } catch (error) {
                // An exception nothing caught ends the invocation, as it does on the real
                // runtime: the handler's work is over whatever is still queued.
                settle(raised(error));
                return;
            }
        }
    }

    globalThis.setTimeout = function (callback, delay) {
        return addTimer(callback, delay, Array.prototype.slice.call(arguments, 2), false);
    };
    globalThis.setInterval = function (callback, delay) {
        return addTimer(callback, delay, Array.prototype.slice.call(arguments, 2), true);
    };
    globalThis.setImmediate = function (callback) {
        return addTimer(callback, 0, Array.prototype.slice.call(arguments, 1), false);
    };
    globalThis.clearTimeout = clearTimer;
    globalThis.clearInterval = clearTimer;
    globalThis.clearImmediate = clearTimer;
    if (typeof globalThis.queueMicrotask !== 'function') {
        globalThis.queueMicrotask = function (callback) {
            Promise.resolve().then(callback);
        };
    }

    // ---------------------------------------------------------------- the process object

    function installProcess(environment) {
        var EventEmitter = requireId('core:events');
        var process = new EventEmitter();
        process.env = environment;
        process.argv = [];
        process.cwd = function () { return TASK_ROOT; };
        process.nextTick = function (callback) {
            var args = Array.prototype.slice.call(arguments, 1);
            Promise.resolve().then(function () { callback.apply(undefined, args); });
        };
        process.exit = function () {
            // "Runtime exited without providing a reason" is what Lambda reports for a
            // runtime that ended the process instead of answering the invocation.
            settle({
                outcome: 'raised',
                errorType: 'Runtime.ExitError',
                errorMessage: 'Runtime exited without providing a reason',
                trace: []
            });
            throw new Error('Runtime exited without providing a reason');
        };
        process.stdout = { write: function (text) { hostLog(String(text)); return true; } };
        process.stderr = { write: function (text) { hostLog(String(text)); return true; } };
        globalThis.process = process;
    }

    // ---------------------------------------------------------------- the invocation

    function raised(error) {
        var type = 'Error';
        var message = '';
        var trace = [];
        if (error instanceof Error) {
            type = error.name || 'Error';
            message = error.message === undefined ? '' : String(error.message);
            if (typeof error.stack === 'string' && error.stack !== '') {
                trace = error.stack.split('\n');
            }
        } else if (error !== null && typeof error === 'object'
            && typeof error.name === 'string' && typeof error.message === 'string') {
            type = error.name;
            message = error.message;
        } else {
            type = typeof error;
            try {
                message = typeof error === 'string' ? error : JSON.stringify(error);
            } catch (ignored) {
                message = String(error);
            }
            if (message === undefined) {
                message = String(error);
            }
        }
        return {
            outcome: 'raised',
            errorType: type,
            errorMessage: message,
            trace: trace
        };
    }

    function runtimeFailure(type, message) {
        return { outcome: 'raised', errorType: type, errorMessage: message, trace: [] };
    }

    function serialized(value) {
        var payload;
        try {
            payload = JSON.stringify(value === undefined ? null : value);
        } catch (error) {
            return raised(error);
        }
        if (payload === undefined) {
            payload = 'null';
        }
        return { outcome: 'returned', payload: payload };
    }

    function settle(outcome) {
        if (state === null || state.settled) {
            return;
        }
        state.settled = true;
        try {
            state.result = JSON.stringify(outcome);
        } catch (error) {
            state.result = JSON.stringify(
                runtimeFailure('Runtime.MarshalError', 'Unable to marshal response: ' + error)
            );
        }
    }

    // The handler string names the module to load and the property path to the function
    // on it: everything up to the first `.` is the module, the rest addresses the export.
    function splitHandler(handler) {
        if (typeof handler !== 'string') {
            return null;
        }
        var cut = handler.indexOf('.');
        if (cut <= 0 || cut === handler.length - 1) {
            return null;
        }
        return { module: handler.slice(0, cut), path: handler.slice(cut + 1).split('.') };
    }

    function property(exported, path) {
        var held = exported;
        for (var index = 0; index < path.length; index += 1) {
            if (held === null || held === undefined) {
                return undefined;
            }
            held = held[path[index]];
        }
        return held;
    }

    function loadHandlerModule(modulePath) {
        // The module the `Handler` names is a path inside the package like any other, so
        // it is normalised and contained the same way before anything is looked up.
        var inside = packagePath('', modulePath);
        var id = inside === null ? null : fileCandidate(inside);
        if (id === null) {
            return Promise.reject(moduleNotFound(modulePath));
        }
        if (endsWith(id, '.mjs') || declaredEsModule(id)) {
            return importModule(id);
        }
        try {
            return Promise.resolve(requireId(id));
        } catch (error) {
            // A file that does not compile as CommonJS because it is written with module
            // syntax is loaded as the ES module it is, which is how a `.js` handler of an
            // ES package is loaded.
            if (error instanceof SyntaxError && !endsWith(id, '.cjs')) {
                return importModule(id).then(null, function (moduleError) {
                    throw moduleError instanceof SyntaxError ? error : moduleError;
                });
            }
            return Promise.reject(error);
        }
    }

    function importModule(id) {
        if (dynamicImport === null) {
            return Promise.reject(new Error("Cannot load ES module '" + id + "'"));
        }
        return dynamicImport('./' + id);
    }

    function makeContext(input) {
        var environment = input.environment;
        var context = {
            callbackWaitsForEmptyEventLoop: true,
            functionName: environment.AWS_LAMBDA_FUNCTION_NAME,
            functionVersion: environment.AWS_LAMBDA_FUNCTION_VERSION,
            invokedFunctionArn: input.invokedFunctionArn,
            memoryLimitInMB: environment.AWS_LAMBDA_FUNCTION_MEMORY_SIZE,
            awsRequestId: input.requestId,
            logGroupName: environment.AWS_LAMBDA_LOG_GROUP_NAME,
            logStreamName: environment.AWS_LAMBDA_LOG_STREAM_NAME,
            identity: undefined,
            clientContext: input.clientContext === null ? undefined : input.clientContext,
            tenantId: input.tenantId === null ? undefined : input.tenantId,
            getRemainingTimeInMillis: function () {
                return Math.max(0, Math.floor(input.deadlineEpochMs - Date.now()));
            }
        };
        return context;
    }

    function finishFromCallback() {
        if (state.callbackError !== null && state.callbackError !== undefined) {
            settle(raised(state.callbackError));
            return;
        }
        settle(serialized(state.callbackValue));
    }

    function callHandler(entry, event, context) {
        var callback = function (error, value) {
            if (state.callbackCalled) {
                return;
            }
            state.callbackCalled = true;
            state.callbackError = error;
            state.callbackValue = value;
            // "callbackWaitsForEmptyEventLoop ... Set to false to send the response right
            // away": otherwise the answer is reported when the event loop is empty.
            if (context.callbackWaitsForEmptyEventLoop === false) {
                finishFromCallback();
            }
        };
        var returned = entry(event, context, callback);
        if (returned !== null && returned !== undefined && typeof returned.then === 'function') {
            // An asynchronous handler's answer is its promise's: the invocation ends when
            // that promise settles.
            state.waiting = 'promise';
            return Promise.resolve(returned).then(serialized, raised);
        }
        // A non-async handler answers through its callback, and the runtime reports that
        // answer once the event loop is empty — `null` when the callback was never called.
        state.waiting = 'loop';
        return new Promise(function () { return undefined; });
    }

    function invoke(input) {
        var split = splitHandler(input.handler);
        if (split === null) {
            return Promise.resolve(
                runtimeFailure('Runtime.MalformedHandlerName', "Bad handler '" + input.handler + "'")
            );
        }
        return loadHandlerModule(split.module).then(function (exported) {
            var entry = property(exported, split.path);
            if (typeof entry !== 'function') {
                return runtimeFailure(
                    'Runtime.HandlerNotFound',
                    input.handler + ' is undefined or not a function'
                );
            }
            var event;
            try {
                event = input.event === null || input.event === undefined || input.event === ''
                    ? {}
                    : JSON.parse(input.event);
            } catch (error) {
                return runtimeFailure('Runtime.InvalidEvent', String(error));
            }
            return callHandler(entry, event, makeContext(input));
        }, function (error) {
            // How the node runtime classifies a module that would not load: a module it
            // could not find, source that would not compile, and anything else the
            // module's own top-level code threw — which is reported as itself.
            if (error !== null && error !== undefined && error.code === 'MODULE_NOT_FOUND') {
                return runtimeFailure(
                    'Runtime.ImportModuleError',
                    'Error: ' + error.message
                );
            }
            if (error instanceof SyntaxError) {
                return runtimeFailure(
                    'Runtime.UserCodeSyntaxError',
                    'SyntaxError: ' + error.message
                );
            }
            return raised(error);
        });
    }

    function start(inputJson) {
        var input = JSON.parse(inputJson);
        state = {
            settled: false,
            result: null,
            waiting: 'promise',
            callbackCalled: false,
            callbackError: null,
            callbackValue: null
        };
        installProcess(input.environment);
        Promise.resolve().then(function () {
            return invoke(input);
        }).then(function (outcome) {
            settle(outcome);
        }, function (error) {
            settle(raised(error));
        });
    }

    function eventLoopEmpty() {
        if (state === null || state.settled || state.waiting !== 'loop') {
            return;
        }
        finishFromCallback();
    }

    globalThis.__lambdaRuntime = Object.freeze({
        start: start,
        poll: function () {
            return state !== null && state.settled ? state.result : null;
        },
        waiting: function () {
            return state === null || state.settled ? '' : state.waiting;
        },
        nextTimerDelayMs: nextTimerDelayMs,
        runDueTimers: runDueTimers,
        eventLoopEmpty: eventLoopEmpty,
        resolveModule: resolveModule,
        moduleSource: moduleSource,
        requireId: requireId
    });
})();
"##;
