// SPDX-License-Identifier: CC0-1.0

//! `os`, `assert`, `querystring` and `url`: the four remaining core modules a handler
//! reaches for that are pure computation over what the run already holds.
//!
//! `os` reports the SANDBOX a function runs in — the task root's writable directory, the
//! memory the invocation was given — rather than anything of the host; `assert`,
//! `querystring` and `url` are algebra over values and text.

/// `os`: what the execution environment reports about itself.
pub(super) const OS_SOURCE: &str = r##"
'use strict';

// The one writable directory a function has, which is what `os.tmpdir()` names.
var TMP = '/tmp';

function memoryBytes() {
    var configured = Number(globalThis.process.env.AWS_LAMBDA_FUNCTION_MEMORY_SIZE);
    return isFinite(configured) && configured > 0 ? configured * 1024 * 1024 : 0;
}

exports.EOL = '\n';
exports.platform = function () { return globalThis.process.platform; };
exports.arch = function () { return globalThis.process.arch; };
exports.type = function () { return 'Linux'; };
exports.release = function () { return globalThis.process.release.name; };
exports.endianness = function () { return 'LE'; };
exports.tmpdir = function () { return TMP; };
exports.homedir = function () { return globalThis.process.env.HOME || TMP; };
exports.hostname = function () { return globalThis.process.env.AWS_LAMBDA_LOG_STREAM_NAME || ''; };
exports.totalmem = memoryBytes;
// What is left of the invocation's own memory allowance: the engine's accounting of what
// this run holds, subtracted from what it was given.
exports.freemem = function () {
    return Math.max(0, memoryBytes() - globalThis.process.memoryUsage().heapUsed);
};
exports.uptime = function () { return globalThis.process.uptime(); };
exports.loadavg = function () { return [0, 0, 0]; };
exports.cpus = function () { return []; };
exports.networkInterfaces = function () { return {}; };
exports.userInfo = function () {
    return {
        uid: 993,
        gid: 990,
        username: 'sbx_user1051',
        homedir: exports.homedir(),
        shell: null
    };
};
exports.constants = { signals: {}, errno: {} };
exports.devNull = '/dev/null';
"##;

/// `assert`: the checks a Node program states its own expectations with.
pub(super) const ASSERT_SOURCE: &str = r##"
'use strict';

var util = require('util');

function AssertionError(options) {
    var error = Error.call(this, options.message);
    this.message = options.message;
    this.actual = options.actual;
    this.expected = options.expected;
    this.operator = options.operator;
    this.generatedMessage = options.generatedMessage === true;
    this.code = 'ERR_ASSERTION';
    this.name = 'AssertionError';
    if (error !== undefined && typeof error.stack === 'string') {
        this.stack = error.stack;
    }
}

AssertionError.prototype = Object.create(Error.prototype);
AssertionError.prototype.constructor = AssertionError;
AssertionError.prototype.name = 'AssertionError';

function failure(actual, expected, message, operator) {
    if (message instanceof Error) {
        throw message;
    }
    throw new AssertionError({
        message: message === undefined || message === null
            ? util.inspect(actual) + ' ' + operator + ' ' + util.inspect(expected)
            : String(message),
        actual: actual,
        expected: expected,
        operator: operator,
        generatedMessage: message === undefined || message === null
    });
}

// Structural equality: the same keys with equal values, compared strictly when `strict`.
function equivalent(first, second, strict) {
    if (strict ? first === second : first == second) {
        return true;
    }
    if (typeof first === 'number' && typeof second === 'number') {
        return Number.isNaN(first) && Number.isNaN(second);
    }
    if (first === null || second === null || typeof first !== 'object' || typeof second !== 'object') {
        return false;
    }
    if (strict && Object.getPrototypeOf(first) !== Object.getPrototypeOf(second)) {
        return false;
    }
    if (Array.isArray(first) !== Array.isArray(second)) {
        return false;
    }
    if (first instanceof Date && second instanceof Date) {
        return first.getTime() === second.getTime();
    }
    var left = Object.keys(first);
    var right = Object.keys(second);
    if (left.length !== right.length) {
        return false;
    }
    for (var index = 0; index < left.length; index += 1) {
        if (!Object.prototype.hasOwnProperty.call(second, left[index])) {
            return false;
        }
        if (!equivalent(first[left[index]], second[left[index]], strict)) {
            return false;
        }
    }
    return true;
}

function ok(value, message) {
    if (!value) {
        failure(value, true, message, '==');
    }
}

var assert = function (value, message) { ok(value, message); };

assert.AssertionError = AssertionError;
assert.ok = ok;
assert.equal = function (actual, expected, message) {
    if (!equivalent(actual, expected, false)) {
        failure(actual, expected, message, '==');
    }
};
assert.notEqual = function (actual, expected, message) {
    if (equivalent(actual, expected, false)) {
        failure(actual, expected, message, '!=');
    }
};
assert.strictEqual = function (actual, expected, message) {
    if (actual !== expected && !(Number.isNaN(actual) && Number.isNaN(expected))) {
        failure(actual, expected, message, 'strictEqual');
    }
};
assert.notStrictEqual = function (actual, expected, message) {
    if (actual === expected) {
        failure(actual, expected, message, 'notStrictEqual');
    }
};
assert.deepEqual = function (actual, expected, message) {
    if (!equivalent(actual, expected, false)) {
        failure(actual, expected, message, 'deepEqual');
    }
};
assert.notDeepEqual = function (actual, expected, message) {
    if (equivalent(actual, expected, false)) {
        failure(actual, expected, message, 'notDeepEqual');
    }
};
assert.deepStrictEqual = function (actual, expected, message) {
    if (!equivalent(actual, expected, true)) {
        failure(actual, expected, message, 'deepStrictEqual');
    }
};
assert.notDeepStrictEqual = function (actual, expected, message) {
    if (equivalent(actual, expected, true)) {
        failure(actual, expected, message, 'notDeepStrictEqual');
    }
};
assert.match = function (text, pattern, message) {
    if (!pattern.test(text)) {
        failure(text, pattern, message, 'match');
    }
};
assert.doesNotMatch = function (text, pattern, message) {
    if (pattern.test(text)) {
        failure(text, pattern, message, 'doesNotMatch');
    }
};
assert.throws = function (run, expected, message) {
    try {
        run();
    } catch (error) {
        return;
    }
    failure(undefined, expected, message === undefined ? 'Missing expected exception.' : message,
        'throws');
};
assert.doesNotThrow = function (run, message) {
    run();
    return undefined;
};
assert.rejects = function (run, expected, message) {
    var started = typeof run === 'function' ? run() : run;
    return Promise.resolve(started).then(
        function () {
            failure(undefined, expected,
                message === undefined ? 'Missing expected rejection.' : message, 'rejects');
        },
        function () { return undefined; }
    );
};
assert.doesNotReject = function (run) {
    var started = typeof run === 'function' ? run() : run;
    return Promise.resolve(started).then(function () { return undefined; });
};
assert.ifError = function (value) {
    if (value !== null && value !== undefined) {
        failure(value, null, 'ifError got unwanted exception: ' + util.inspect(value), 'ifError');
    }
};
assert.fail = function (message) {
    failure(undefined, undefined,
        message === undefined ? 'Failed' : message, 'fail');
};
assert.strict = assert;

module.exports = assert;
"##;

/// `querystring`: the legacy query codec.
pub(super) const QUERYSTRING_SOURCE: &str = r##"
'use strict';

function escapeText(text) {
    return encodeURIComponent(String(text)).replace(/%20/g, '+');
}

function unescapeText(text) {
    try {
        return decodeURIComponent(String(text).replace(/\+/g, ' '));
    } catch (error) {
        return String(text);
    }
}

exports.escape = escapeText;
exports.unescape = unescapeText;

exports.parse = function (text, separator, equals) {
    var parsed = Object.create(null);
    if (typeof text !== 'string' || text === '') {
        return parsed;
    }
    var pairSeparator = separator === undefined ? '&' : separator;
    var valueSeparator = equals === undefined ? '=' : equals;
    var pairs = text.split(pairSeparator);
    for (var index = 0; index < pairs.length; index += 1) {
        if (pairs[index] === '') {
            continue;
        }
        var cut = pairs[index].indexOf(valueSeparator);
        var name = cut < 0 ? pairs[index] : pairs[index].slice(0, cut);
        var value = cut < 0 ? '' : pairs[index].slice(cut + valueSeparator.length);
        var key = unescapeText(name);
        var held = unescapeText(value);
        if (parsed[key] === undefined) {
            parsed[key] = held;
        } else if (Array.isArray(parsed[key])) {
            parsed[key].push(held);
        } else {
            parsed[key] = [parsed[key], held];
        }
    }
    return parsed;
};

exports.stringify = function (values, separator, equals) {
    var pairSeparator = separator === undefined ? '&' : separator;
    var valueSeparator = equals === undefined ? '=' : equals;
    if (values === null || typeof values !== 'object') {
        return '';
    }
    var out = [];
    var keys = Object.keys(values);
    for (var index = 0; index < keys.length; index += 1) {
        var value = values[keys[index]];
        var name = escapeText(keys[index]);
        if (Array.isArray(value)) {
            for (var at = 0; at < value.length; at += 1) {
                out.push(name + valueSeparator + escapeText(value[at]));
            }
            continue;
        }
        out.push(name + valueSeparator + escapeText(value === undefined || value === null ? '' : value));
    }
    return out.join(pairSeparator);
};

exports.decode = exports.parse;
exports.encode = exports.stringify;
"##;

/// `url`: the URL object, its search parameters, and the two file-URL conversions an ES
/// module resolves its own siblings with.
pub(super) const URL_SOURCE: &str = r##"
'use strict';

var querystring = require('querystring');

function URLSearchParams(init) {
    this._pairs = [];
    if (typeof init === 'string') {
        var parsed = querystring.parse(init.charAt(0) === '?' ? init.slice(1) : init);
        var names = Object.keys(parsed);
        for (var index = 0; index < names.length; index += 1) {
            var held = parsed[names[index]];
            if (Array.isArray(held)) {
                for (var at = 0; at < held.length; at += 1) {
                    this._pairs.push([names[index], held[at]]);
                }
            } else {
                this._pairs.push([names[index], held]);
            }
        }
    } else if (Array.isArray(init)) {
        for (var pair = 0; pair < init.length; pair += 1) {
            this._pairs.push([String(init[pair][0]), String(init[pair][1])]);
        }
    } else if (init !== null && typeof init === 'object') {
        var keys = Object.keys(init);
        for (var key = 0; key < keys.length; key += 1) {
            this._pairs.push([keys[key], String(init[keys[key]])]);
        }
    }
}

URLSearchParams.prototype.append = function (name, value) {
    this._pairs.push([String(name), String(value)]);
};
URLSearchParams.prototype.set = function (name, value) {
    var text = String(name);
    var kept = this._pairs.filter(function (pair) { return pair[0] !== text; });
    kept.push([text, String(value)]);
    this._pairs = kept;
};
URLSearchParams.prototype.get = function (name) {
    var text = String(name);
    for (var index = 0; index < this._pairs.length; index += 1) {
        if (this._pairs[index][0] === text) {
            return this._pairs[index][1];
        }
    }
    return null;
};
URLSearchParams.prototype.getAll = function (name) {
    var text = String(name);
    return this._pairs
        .filter(function (pair) { return pair[0] === text; })
        .map(function (pair) { return pair[1]; });
};
URLSearchParams.prototype.has = function (name) {
    return this.get(name) !== null;
};
URLSearchParams.prototype.delete = function (name) {
    var text = String(name);
    this._pairs = this._pairs.filter(function (pair) { return pair[0] !== text; });
};
URLSearchParams.prototype.forEach = function (visit, bound) {
    for (var index = 0; index < this._pairs.length; index += 1) {
        visit.call(bound, this._pairs[index][1], this._pairs[index][0], this);
    }
};
URLSearchParams.prototype.keys = function () {
    return this._pairs.map(function (pair) { return pair[0]; })[Symbol.iterator]();
};
URLSearchParams.prototype.values = function () {
    return this._pairs.map(function (pair) { return pair[1]; })[Symbol.iterator]();
};
URLSearchParams.prototype.entries = function () {
    return this._pairs.map(function (pair) { return pair.slice(); })[Symbol.iterator]();
};
URLSearchParams.prototype[Symbol.iterator] = URLSearchParams.prototype.entries;
URLSearchParams.prototype.toString = function () {
    return this._pairs
        .map(function (pair) {
            return querystring.escape(pair[0]) + '=' + querystring.escape(pair[1]);
        })
        .join('&');
};
Object.defineProperty(URLSearchParams.prototype, 'size', {
    get: function () { return this._pairs.length; }
});

// The generic form of an absolute URL: scheme, optional authority, path, query, fragment.
var ABSOLUTE = /^([A-Za-z][A-Za-z0-9+.-]*):(\/\/([^/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/;

function URL(input, base) {
    var text = String(input);
    var matched = ABSOLUTE.exec(text);
    if (matched === null) {
        if (base === undefined || base === null) {
            var error = new TypeError('Invalid URL');
            error.code = 'ERR_INVALID_URL';
            error.input = text;
            throw error;
        }
        var parent = base instanceof URL ? base : new URL(base);
        var joined = text.charAt(0) === '/'
            ? text
            : parent.pathname.replace(/[^/]*$/, '') + text;
        return new URL(parent.protocol + '//' + parent.host + joined);
    }
    var authority = matched[3] === undefined ? '' : matched[3];
    var credentials = authority.indexOf('@') < 0 ? '' : authority.slice(0, authority.indexOf('@'));
    var host = authority.indexOf('@') < 0 ? authority : authority.slice(authority.indexOf('@') + 1);
    var colon = host.lastIndexOf(':');
    this.protocol = matched[1].toLowerCase() + ':';
    this.username = credentials.indexOf(':') < 0 ? credentials : credentials.slice(0, credentials.indexOf(':'));
    this.password = credentials.indexOf(':') < 0 ? '' : credentials.slice(credentials.indexOf(':') + 1);
    this.hostname = colon > 0 ? host.slice(0, colon) : host;
    this.port = colon > 0 ? host.slice(colon + 1) : '';
    this.host = host;
    this.pathname = matched[4] === undefined || matched[4] === '' ? (authority === '' ? '' : '/') : matched[4];
    this.search = matched[5] === undefined ? '' : matched[5];
    this.hash = matched[6] === undefined ? '' : matched[6];
    this.searchParams = new URLSearchParams(this.search);
    this.origin = authority === '' ? 'null' : this.protocol + '//' + host;
    this.href = this.toString();
}

URL.prototype.toString = function () {
    var credentials = this.username === '' ? '' : this.username
        + (this.password === '' ? '' : ':' + this.password) + '@';
    var authority = this.host === '' ? '' : '//' + credentials + this.host;
    var query = this.searchParams.toString();
    return this.protocol + authority + this.pathname
        + (query === '' ? '' : '?' + query) + this.hash;
};
URL.prototype.toJSON = function () { return this.toString(); };

function fileURLToPath(url) {
    var held = url instanceof URL ? url : new URL(String(url));
    if (held.protocol !== 'file:') {
        var error = new TypeError('The URL must be of scheme file');
        error.code = 'ERR_INVALID_URL_SCHEME';
        throw error;
    }
    return decodeURIComponent(held.pathname);
}

function pathToFileURL(path) {
    var text = String(path);
    var absolute = text.charAt(0) === '/' ? text : '/' + text;
    return new URL('file://' + absolute.split('/').map(encodeURIComponent).join('/'));
}

exports.URL = URL;
exports.URLSearchParams = URLSearchParams;
exports.fileURLToPath = fileURLToPath;
exports.pathToFileURL = pathToFileURL;
exports.parse = function (text) {
    var held = new URL(String(text));
    return {
        protocol: held.protocol,
        host: held.host,
        hostname: held.hostname,
        port: held.port === '' ? null : held.port,
        pathname: held.pathname,
        search: held.search === '' ? null : held.search,
        query: held.search === '' ? null : held.search.slice(1),
        hash: held.hash === '' ? null : held.hash,
        href: held.href,
        path: held.pathname + held.search
    };
};
exports.format = function (held) {
    return held instanceof URL ? held.toString() : String(held.href);
};
exports.resolve = function (base, target) {
    return new URL(String(target), String(base)).toString();
};
"##;
