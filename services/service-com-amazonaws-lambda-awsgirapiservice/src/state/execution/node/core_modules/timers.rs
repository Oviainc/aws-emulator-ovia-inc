// SPDX-License-Identifier: CC0-1.0

//! `timers` and `timers/promises`: the scheduling calls, as modules.
//!
//! The queue itself is the runtime's and the clock it is driven by is the HOST's
//! ([`super::super::super::node`]'s drive loop), so these modules only name the global
//! functions that queue work; nothing here waits, and no timer can hold a run past its
//! deadline.

/// `timers`: the global scheduling functions under their module names.
pub(super) const TIMERS_SOURCE: &str = r##"
'use strict';

exports.setTimeout = function () {
    return globalThis.setTimeout.apply(globalThis, arguments);
};
exports.clearTimeout = function (handle) { return globalThis.clearTimeout(handle); };
exports.setInterval = function () {
    return globalThis.setInterval.apply(globalThis, arguments);
};
exports.clearInterval = function (handle) { return globalThis.clearInterval(handle); };
exports.setImmediate = function () {
    return globalThis.setImmediate.apply(globalThis, arguments);
};
exports.clearImmediate = function (handle) { return globalThis.clearImmediate(handle); };
exports.promises = require('timers/promises');
"##;

/// `timers/promises`: the promise form of the same queue.
pub(super) const TIMERS_PROMISES_SOURCE: &str = r##"
'use strict';

// An abort that has already happened is reported before anything is queued, and one that
// happens while the timer is pending rejects the promise the way Node's does.
function abortReason(signal) {
    var reason = signal.reason;
    if (reason !== undefined && reason !== null) {
        return reason;
    }
    var error = new Error('The operation was aborted');
    error.name = 'AbortError';
    error.code = 'ABORT_ERR';
    return error;
}

function scheduled(queue, cancel, delay, value, options) {
    var signal = options !== null && typeof options === 'object' ? options.signal : undefined;
    return new Promise(function (resolve, reject) {
        if (signal !== undefined && signal !== null && signal.aborted === true) {
            reject(abortReason(signal));
            return;
        }
        var handle = queue(function () { resolve(value); }, delay);
        if (signal !== undefined && signal !== null && typeof signal.addEventListener === 'function') {
            signal.addEventListener('abort', function () {
                cancel(handle);
                reject(abortReason(signal));
            });
        }
    });
}

exports.setTimeout = function (delay, value, options) {
    return scheduled(globalThis.setTimeout, globalThis.clearTimeout, delay, value, options);
};
exports.setImmediate = function (value, options) {
    return scheduled(
        function (callback) { return globalThis.setImmediate(callback); },
        globalThis.clearImmediate,
        0,
        value,
        options
    );
};
exports.setInterval = function (delay, value, options) {
    // "Returns an async iterator that generates values in an interval of delay ms": each
    // turn is one wait of the same queue, so the iterator holds no timer between turns.
    return {
        [Symbol.asyncIterator]: function () {
            return {
                next: function () {
                    return exports.setTimeout(delay, value, options).then(function (held) {
                        return { value: held, done: false };
                    });
                },
                return: function () {
                    return Promise.resolve({ value: undefined, done: true });
                }
            };
        }
    };
};
exports.scheduler = {
    wait: function (delay, options) { return exports.setTimeout(delay, undefined, options); },
    yield: function () { return exports.setImmediate(); }
};
"##;
