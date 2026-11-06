// SPDX-License-Identifier: CC0-1.0

//! The node CORE MODULES a handler run in this emulator can import, each as the JavaScript
//! source of a CommonJS module.
//!
//! A deployment package's own files are the caller's; these are the ones a Node program
//! expects of its runtime rather than of its package. They are loaded by the very
//! mechanism a package file is ([`super::runtime`]'s `require`), so a core module is
//! compiled, cached and reachable under both its bare and its `node:` name exactly as a
//! package module is.
//!
//! Every one of them is COMPUTATION over values the run already holds: text, paths,
//! formatting and event dispatch. None reaches a host file, socket, process or
//! environment, because there is no host function here to reach one with — the whole host
//! surface of a run is the three functions [`super::install_host`] installs. A module a
//! Node program expects and this emulator does not supply is simply not resolvable, so a
//! handler that needs one fails to import exactly as it would if the dependency were
//! missing from its package; nothing here pretends to be a module it is not.

/// Every core module, by the name a handler imports it under.
pub(super) const CORE_MODULES: &[(&str, &str)] = &[
    ("path", PATH_SOURCE),
    ("util", UTIL_SOURCE),
    ("events", EVENTS_SOURCE),
];

/// The source of the core module `name` names, or `None` when this emulator supplies none.
///
/// The `node:` prefix a modern import spells a core module with resolves to the same
/// module as the bare name.
#[must_use]
pub(super) fn core_module_source(name: &str) -> Option<&'static str> {
    let bare = name.strip_prefix("node:").unwrap_or(name);
    CORE_MODULES
        .iter()
        .find(|(module, _)| *module == bare)
        .map(|(_, source)| *source)
}

/// `path`: the POSIX path algebra, over strings alone.
///
/// The working directory a relative path resolves against is the task root the runtime
/// reports (`LAMBDA_TASK_ROOT`), because that is the directory a function's package is at:
/// no host directory is consulted, and no path here is opened.
const PATH_SOURCE: &str = r##"
'use strict';

var TASK_ROOT = '/var/task';

function assertPath(value) {
    if (typeof value !== 'string') {
        throw new TypeError('Path must be a string. Received ' + JSON.stringify(value));
    }
}

function normalizeSegments(segments, allowAboveRoot) {
    var out = [];
    for (var index = 0; index < segments.length; index += 1) {
        var segment = segments[index];
        if (segment === '' || segment === '.') {
            continue;
        }
        if (segment === '..') {
            if (out.length > 0 && out[out.length - 1] !== '..') {
                out.pop();
            } else if (allowAboveRoot) {
                out.push('..');
            }
            continue;
        }
        out.push(segment);
    }
    return out;
}

function normalize(path) {
    assertPath(path);
    if (path === '') {
        return '.';
    }
    var absolute = path.charAt(0) === '/';
    var trailing = path.length > 1 && path.charAt(path.length - 1) === '/';
    var joined = normalizeSegments(path.split('/'), !absolute).join('/');
    if (joined === '') {
        return absolute ? '/' : (trailing ? './' : '.');
    }
    if (trailing) {
        joined = joined + '/';
    }
    return absolute ? '/' + joined : joined;
}

function join() {
    var parts = [];
    for (var index = 0; index < arguments.length; index += 1) {
        assertPath(arguments[index]);
        if (arguments[index] !== '') {
            parts.push(arguments[index]);
        }
    }
    if (parts.length === 0) {
        return '.';
    }
    return normalize(parts.join('/'));
}

function resolve() {
    var resolved = '';
    var absolute = false;
    for (var index = arguments.length - 1; index >= 0 && !absolute; index -= 1) {
        var part = arguments[index];
        assertPath(part);
        if (part === '') {
            continue;
        }
        resolved = resolved === '' ? part : part + '/' + resolved;
        absolute = part.charAt(0) === '/';
    }
    if (!absolute) {
        resolved = resolved === '' ? TASK_ROOT : TASK_ROOT + '/' + resolved;
    }
    return '/' + normalizeSegments(resolved.split('/'), false).join('/');
}

function dirname(path) {
    assertPath(path);
    var normalized = path.length > 1 ? path.replace(/\/+$/, '') : path;
    var cut = normalized.lastIndexOf('/');
    if (cut < 0) {
        return '.';
    }
    if (cut === 0) {
        return '/';
    }
    return normalized.slice(0, cut);
}

function basename(path, suffix) {
    assertPath(path);
    var normalized = path.length > 1 ? path.replace(/\/+$/, '') : path;
    var cut = normalized.lastIndexOf('/');
    var name = cut < 0 ? normalized : normalized.slice(cut + 1);
    if (typeof suffix === 'string' && suffix !== '' && name !== suffix) {
        if (name.length >= suffix.length && name.slice(name.length - suffix.length) === suffix) {
            name = name.slice(0, name.length - suffix.length);
        }
    }
    return name;
}

function extname(path) {
    var name = basename(path);
    var cut = name.lastIndexOf('.');
    if (cut <= 0) {
        return '';
    }
    return name.slice(cut);
}

function isAbsolute(path) {
    assertPath(path);
    return path.charAt(0) === '/';
}

function relative(from, to) {
    var fromParts = resolve(from).split('/');
    var toParts = resolve(to).split('/');
    var shared = 0;
    while (shared < fromParts.length && shared < toParts.length
        && fromParts[shared] === toParts[shared]) {
        shared += 1;
    }
    var up = [];
    for (var index = shared; index < fromParts.length; index += 1) {
        if (fromParts[index] !== '') {
            up.push('..');
        }
    }
    return up.concat(toParts.slice(shared)).join('/');
}

function parse(path) {
    assertPath(path);
    var base = basename(path);
    var ext = extname(path);
    return {
        root: isAbsolute(path) ? '/' : '',
        dir: dirname(path),
        base: base,
        ext: ext,
        name: ext === '' ? base : base.slice(0, base.length - ext.length)
    };
}

function format(parsed) {
    if (parsed === null || typeof parsed !== 'object') {
        throw new TypeError('The "pathObject" argument must be of type object.');
    }
    var dir = parsed.dir || parsed.root || '';
    var base = parsed.base || ((parsed.name || '') + (parsed.ext || ''));
    if (dir === '') {
        return base;
    }
    return dir === parsed.root ? dir + base : dir + '/' + base;
}

var path = {
    normalize: normalize,
    join: join,
    resolve: resolve,
    dirname: dirname,
    basename: basename,
    extname: extname,
    isAbsolute: isAbsolute,
    relative: relative,
    parse: parse,
    format: format,
    sep: '/',
    delimiter: ':'
};
path.posix = path;
module.exports = path;
"##;

/// `util`: the formatting and promise adapters a Node program reads values with.
const UTIL_SOURCE: &str = r##"
'use strict';

function quoted(text) {
    return "'" + text.replace(/\\/g, '\\\\').replace(/'/g, "\\'").replace(/\n/g, '\\n') + "'";
}

function inspect(value, seen) {
    seen = seen || [];
    if (value === undefined) {
        return 'undefined';
    }
    if (value === null) {
        return 'null';
    }
    var kind = typeof value;
    if (kind === 'string') {
        return quoted(value);
    }
    if (kind === 'number' || kind === 'boolean' || kind === 'symbol') {
        return String(value);
    }
    if (kind === 'bigint') {
        return String(value) + 'n';
    }
    if (kind === 'function') {
        return value.name ? '[Function: ' + value.name + ']' : '[Function (anonymous)]';
    }
    if (value instanceof Error) {
        return typeof value.stack === 'string' && value.stack !== ''
            ? value.stack
            : value.name + ': ' + value.message;
    }
    if (value instanceof Date) {
        return value.toISOString();
    }
    if (value instanceof RegExp) {
        return String(value);
    }
    for (var index = 0; index < seen.length; index += 1) {
        if (seen[index] === value) {
            return '[Circular *1]';
        }
    }
    seen = seen.concat([value]);
    if (Array.isArray(value)) {
        var items = [];
        for (var element = 0; element < value.length; element += 1) {
            items.push(inspect(value[element], seen));
        }
        return items.length === 0 ? '[]' : '[ ' + items.join(', ') + ' ]';
    }
    if (typeof Map === 'function' && value instanceof Map) {
        var entries = [];
        value.forEach(function (held, key) {
            entries.push(inspect(key, seen) + ' => ' + inspect(held, seen));
        });
        return 'Map(' + value.size + ') {' + (entries.length ? ' ' + entries.join(', ') + ' ' : '') + '}';
    }
    if (typeof Set === 'function' && value instanceof Set) {
        var members = [];
        value.forEach(function (held) {
            members.push(inspect(held, seen));
        });
        return 'Set(' + value.size + ') {' + (members.length ? ' ' + members.join(', ') + ' ' : '') + '}';
    }
    var keys = Object.keys(value);
    var pairs = [];
    for (var key = 0; key < keys.length; key += 1) {
        pairs.push(keys[key] + ': ' + inspect(value[keys[key]], seen));
    }
    var prefix = '';
    if (value.constructor && value.constructor.name && value.constructor.name !== 'Object') {
        prefix = value.constructor.name + ' ';
    }
    return pairs.length === 0 ? prefix + '{}' : prefix + '{ ' + pairs.join(', ') + ' }';
}

function format() {
    var args = Array.prototype.slice.call(arguments);
    var out = [];
    if (typeof args[0] === 'string' && args[0].indexOf('%') >= 0) {
        var template = args.shift();
        var rendered = '';
        var index = 0;
        while (index < template.length) {
            var character = template.charAt(index);
            if (character !== '%' || index + 1 >= template.length) {
                rendered = rendered + character;
                index += 1;
                continue;
            }
            var directive = template.charAt(index + 1);
            index += 2;
            if (directive === '%') {
                rendered = rendered + '%';
            } else if (directive === 's') {
                var next = args.shift();
                rendered = rendered + (typeof next === 'string' ? next : inspect(next));
            } else if (directive === 'd' || directive === 'i') {
                rendered = rendered + String(parseInt(args.shift(), 10));
            } else if (directive === 'f') {
                rendered = rendered + String(parseFloat(args.shift()));
            } else if (directive === 'j') {
                try {
                    rendered = rendered + JSON.stringify(args.shift());
                } catch (error) {
                    rendered = rendered + '[Circular]';
                }
            } else if (directive === 'o' || directive === 'O') {
                rendered = rendered + inspect(args.shift());
            } else {
                rendered = rendered + '%' + directive;
            }
        }
        out.push(rendered);
    }
    for (var rest = 0; rest < args.length; rest += 1) {
        out.push(typeof args[rest] === 'string' ? args[rest] : inspect(args[rest]));
    }
    return out.join(' ');
}

function promisify(original) {
    if (typeof original !== 'function') {
        throw new TypeError('The "original" argument must be of type function.');
    }
    return function () {
        var args = Array.prototype.slice.call(arguments);
        var self = this;
        return new Promise(function (resolve, reject) {
            args.push(function (error, value) {
                if (error) {
                    reject(error);
                } else {
                    resolve(value);
                }
            });
            try {
                original.apply(self, args);
            } catch (error) {
                reject(error);
            }
        });
    };
}

function callbackify(original) {
    if (typeof original !== 'function') {
        throw new TypeError('The "original" argument must be of type function.');
    }
    return function () {
        var args = Array.prototype.slice.call(arguments);
        var callback = args.pop();
        Promise.resolve(original.apply(this, args)).then(function (value) {
            callback(null, value);
        }, function (error) {
            callback(error || new Error('Promise was rejected with a falsy value'));
        });
    };
}

function inherits(constructor, parent) {
    constructor.super_ = parent;
    constructor.prototype = Object.create(parent.prototype, {
        constructor: {
            value: constructor,
            enumerable: false,
            writable: true,
            configurable: true
        }
    });
}

module.exports = {
    format: format,
    inspect: inspect,
    promisify: promisify,
    callbackify: callbackify,
    inherits: inherits,
    deprecate: function (fn) { return fn; },
    types: {
        isPromise: function (value) {
            return !!value && typeof value.then === 'function';
        },
        isDate: function (value) { return value instanceof Date; },
        isRegExp: function (value) { return value instanceof RegExp; },
        isMap: function (value) { return typeof Map === 'function' && value instanceof Map; },
        isSet: function (value) { return typeof Set === 'function' && value instanceof Set; }
    }
};
"##;

/// `events`: the `EventEmitter` a Node program dispatches its own events through.
const EVENTS_SOURCE: &str = r##"
'use strict';

function EventEmitter() {
    this._events = Object.create(null);
    this._maxListeners = 10;
}

EventEmitter.prototype._listenersFor = function (type) {
    if (!this._events) {
        this._events = Object.create(null);
    }
    if (!this._events[type]) {
        this._events[type] = [];
    }
    return this._events[type];
};

EventEmitter.prototype.addListener = function (type, listener) {
    if (typeof listener !== 'function') {
        throw new TypeError('The "listener" argument must be of type function.');
    }
    this._listenersFor(type).push(listener);
    return this;
};

EventEmitter.prototype.on = EventEmitter.prototype.addListener;

EventEmitter.prototype.prependListener = function (type, listener) {
    if (typeof listener !== 'function') {
        throw new TypeError('The "listener" argument must be of type function.');
    }
    this._listenersFor(type).unshift(listener);
    return this;
};

EventEmitter.prototype.once = function (type, listener) {
    var self = this;
    var wrapper = function () {
        self.removeListener(type, wrapper);
        listener.apply(self, Array.prototype.slice.call(arguments));
    };
    wrapper.listener = listener;
    return this.on(type, wrapper);
};

EventEmitter.prototype.removeListener = function (type, listener) {
    var listeners = this._listenersFor(type);
    for (var index = 0; index < listeners.length; index += 1) {
        if (listeners[index] === listener || listeners[index].listener === listener) {
            listeners.splice(index, 1);
            break;
        }
    }
    return this;
};

EventEmitter.prototype.off = EventEmitter.prototype.removeListener;

EventEmitter.prototype.removeAllListeners = function (type) {
    if (type === undefined) {
        this._events = Object.create(null);
    } else {
        this._events[type] = [];
    }
    return this;
};

EventEmitter.prototype.listeners = function (type) {
    return this._listenersFor(type).slice();
};

EventEmitter.prototype.listenerCount = function (type) {
    return this._listenersFor(type).length;
};

EventEmitter.prototype.eventNames = function () {
    return Object.keys(this._events || {});
};

EventEmitter.prototype.setMaxListeners = function (count) {
    this._maxListeners = count;
    return this;
};

EventEmitter.prototype.getMaxListeners = function () {
    return this._maxListeners;
};

EventEmitter.prototype.emit = function (type) {
    var args = Array.prototype.slice.call(arguments, 1);
    var listeners = this._listenersFor(type).slice();
    for (var index = 0; index < listeners.length; index += 1) {
        listeners[index].apply(this, args);
    }
    return listeners.length > 0;
};

EventEmitter.EventEmitter = EventEmitter;
EventEmitter.defaultMaxListeners = 10;
module.exports = EventEmitter;
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every core module is reachable under its bare name and under the `node:` name a
    /// modern import spells it with, and nothing else resolves to a module.
    #[test]
    fn a_core_module_resolves_under_both_of_its_names() {
        for (name, _) in CORE_MODULES {
            for spelled in [(*name).to_string(), format!("node:{name}")] {
                assert!(
                    core_module_source(&spelled).is_some(),
                    "case {spelled:?}: expected the core module's source, got none"
                );
            }
        }
        for absent in ["fs", "net", "child_process", "node:fs", "os", "", "node:"] {
            assert!(
                core_module_source(absent).is_none(),
                "case {absent:?}: this emulator supplies no such core module, so none must \
                 resolve"
            );
        }
    }
}
