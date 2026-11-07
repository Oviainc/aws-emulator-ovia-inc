// SPDX-License-Identifier: CC0-1.0

//! `stream` and `string_decoder`: the stream shapes a Node program composes, over the
//! event emitter the runtime already supplies.
//!
//! These are the OBJECT model — a writable sink, a readable source, the transform between
//! them and the `pipeline` that joins them — and nothing else: a stream here moves values
//! inside the engine, and the only sink that leaves it is the one the runtime hands a
//! response-streaming handler, which the host writes.

/// `stream`: `Readable`, `Writable`, `Duplex`, `Transform`, `PassThrough` and `pipeline`.
pub(super) const STREAM_SOURCE: &str = r##"
'use strict';

var EventEmitter = require('events');

function inherit(child, parent) {
    child.prototype = Object.create(parent.prototype);
    child.prototype.constructor = child;
}

function Writable(options) {
    EventEmitter.call(this);
    this.writable = true;
    this.writableEnded = false;
    this.writableFinished = false;
    this.destroyed = false;
    this._options = options === undefined || options === null ? {} : options;
    if (typeof this._options.write === 'function') {
        this._write = this._options.write;
    }
    if (typeof this._options.final === 'function') {
        this._final = this._options.final;
    }
}

inherit(Writable, EventEmitter);

Writable.prototype._write = function (chunk, encoding, done) {
    done();
};

Writable.prototype.write = function (chunk, encoding, callback) {
    var done = typeof encoding === 'function' ? encoding : callback;
    var named = typeof encoding === 'function' ? undefined : encoding;
    if (this.writableEnded) {
        var error = new Error('write after end');
        error.code = 'ERR_STREAM_WRITE_AFTER_END';
        this.emit('error', error);
        return false;
    }
    var self = this;
    this._write(chunk, named, function (error) {
        if (error !== undefined && error !== null) {
            self.emit('error', error);
        }
        if (typeof done === 'function') {
            done(error === undefined ? null : error);
        }
    });
    return true;
};

Writable.prototype.end = function (chunk, encoding, callback) {
    var done = typeof chunk === 'function' ? chunk
        : (typeof encoding === 'function' ? encoding : callback);
    if (chunk !== undefined && chunk !== null && typeof chunk !== 'function') {
        this.write(chunk, typeof encoding === 'function' ? undefined : encoding);
    }
    if (this.writableEnded) {
        return this;
    }
    this.writableEnded = true;
    var self = this;
    var finish = function () {
        self.writableFinished = true;
        self.writable = false;
        self.emit('finish');
        self.emit('close');
        if (typeof done === 'function') {
            done();
        }
    };
    if (typeof this._final === 'function') {
        this._final(function () { finish(); });
        return this;
    }
    finish();
    return this;
};

Writable.prototype.setDefaultEncoding = function () { return this; };
Writable.prototype.cork = function () { return undefined; };
Writable.prototype.uncork = function () { return undefined; };

Writable.prototype.destroy = function (error) {
    this.destroyed = true;
    this.writable = false;
    if (error !== undefined && error !== null) {
        this.emit('error', error);
    }
    this.emit('close');
    return this;
};

function Readable(options) {
    EventEmitter.call(this);
    this.readable = true;
    this.readableEnded = false;
    this.destroyed = false;
    this._queue = [];
    this._ended = false;
    this._flowing = false;
    this._options = options === undefined || options === null ? {} : options;
    if (typeof this._options.read === 'function') {
        this._read = this._options.read;
    }
}

inherit(Readable, EventEmitter);

Readable.prototype._read = function () { return undefined; };

Readable.prototype.push = function (chunk) {
    if (chunk === null) {
        this._ended = true;
        this._drain();
        return false;
    }
    this._queue.push(chunk);
    this._drain();
    return true;
};

Readable.prototype._drain = function () {
    if (!this._flowing) {
        return;
    }
    while (this._queue.length > 0) {
        this.emit('data', this._queue.shift());
    }
    if (this._ended && !this.readableEnded) {
        this.readableEnded = true;
        this.readable = false;
        this.emit('end');
        this.emit('close');
    }
};

Readable.prototype.read = function () {
    if (this._queue.length === 0) {
        this._read();
    }
    return this._queue.length === 0 ? null : this._queue.shift();
};

Readable.prototype.on = function (event, listener) {
    var added = EventEmitter.prototype.on.call(this, event, listener);
    if (event === 'data') {
        this.resume();
    }
    return added;
};

Readable.prototype.resume = function () {
    if (this._flowing) {
        return this;
    }
    this._flowing = true;
    var self = this;
    Promise.resolve().then(function () {
        self._read();
        self._drain();
    });
    return this;
};

Readable.prototype.pause = function () {
    this._flowing = false;
    return this;
};

Readable.prototype.pipe = function (destination, options) {
    var self = this;
    this.on('data', function (chunk) { destination.write(chunk); });
    this.on('end', function () {
        if (options === undefined || options === null || options.end !== false) {
            destination.end();
        }
    });
    this.on('error', function (error) { destination.emit('error', error); });
    this.resume();
    return destination;
};

Readable.prototype.destroy = function (error) {
    this.destroyed = true;
    this.readable = false;
    if (error !== undefined && error !== null) {
        this.emit('error', error);
    }
    this.emit('close');
    return this;
};

Readable.prototype.setEncoding = function (encoding) {
    this._encoding = encoding;
    return this;
};

Readable.prototype[Symbol.asyncIterator] = function () {
    var self = this;
    var waiting = [];
    var done = false;
    var failure = null;
    this.on('data', function (chunk) { waiting.push(chunk); });
    this.on('end', function () { done = true; });
    this.on('error', function (error) { failure = error; done = true; });
    var pull = function () {
        if (waiting.length > 0) {
            return Promise.resolve({ value: waiting.shift(), done: false });
        }
        if (failure !== null) {
            return Promise.reject(failure);
        }
        if (done) {
            return Promise.resolve({ value: undefined, done: true });
        }
        return new Promise(function (resolve) {
            globalThis.setImmediate(function () { resolve(pull()); });
        });
    };
    return { next: pull, return: function () {
        self.destroy();
        return Promise.resolve({ value: undefined, done: true });
    } };
};

// "Readable.from(iterable)": the values of an iterable, as a readable source.
Readable.from = function (iterable) {
    var held = new Readable();
    var pushed = false;
    held._read = function () {
        if (pushed) {
            return;
        }
        pushed = true;
        Promise.resolve().then(function () {
            var pump = function (iterator) {
                var step = iterator.next();
                return Promise.resolve(step).then(function (settled) {
                    if (settled.done) {
                        held.push(null);
                        return undefined;
                    }
                    return Promise.resolve(settled.value).then(function (value) {
                        held.push(value);
                        return pump(iterator);
                    });
                });
            };
            var iterator = typeof iterable[Symbol.asyncIterator] === 'function'
                ? iterable[Symbol.asyncIterator]()
                : iterable[Symbol.iterator]();
            return pump(iterator);
        });
    };
    return held;
};

function Duplex(options) {
    Readable.call(this, options);
    Writable.call(this, options);
}

inherit(Duplex, Readable);
var writableMembers = Object.keys(Writable.prototype).concat([
    'write', 'end', 'destroy', 'setDefaultEncoding', 'cork', 'uncork', '_write'
]);
for (var index = 0; index < writableMembers.length; index += 1) {
    if (Writable.prototype[writableMembers[index]] !== undefined
        && writableMembers[index] !== 'destroy') {
        Duplex.prototype[writableMembers[index]] = Writable.prototype[writableMembers[index]];
    }
}

function Transform(options) {
    Duplex.call(this, options);
    if (options !== undefined && options !== null && typeof options.transform === 'function') {
        this._transform = options.transform;
    }
}

inherit(Transform, Duplex);
var duplexMembers = ['write', 'end', 'setDefaultEncoding', 'cork', 'uncork'];
for (var at = 0; at < duplexMembers.length; at += 1) {
    Transform.prototype[duplexMembers[at]] = Duplex.prototype[duplexMembers[at]];
}

Transform.prototype._transform = function (chunk, encoding, done) {
    done(null, chunk);
};

Transform.prototype._write = function (chunk, encoding, done) {
    var self = this;
    this._transform(chunk, encoding, function (error, transformed) {
        if (error === undefined || error === null) {
            if (transformed !== undefined && transformed !== null) {
                self.push(transformed);
            }
        }
        done(error);
    });
};

Transform.prototype._final = function (done) {
    this.push(null);
    done();
};

function PassThrough(options) {
    Transform.call(this, options);
}

inherit(PassThrough, Transform);
for (var member = 0; member < duplexMembers.length; member += 1) {
    PassThrough.prototype[duplexMembers[member]] = Transform.prototype[duplexMembers[member]];
}
PassThrough.prototype._write = Transform.prototype._write;
PassThrough.prototype._transform = Transform.prototype._transform;
PassThrough.prototype._final = Transform.prototype._final;

function finished(stream, callback) {
    var answer = new Promise(function (resolve, reject) {
        var settled = false;
        var settle = function (error) {
            if (settled) {
                return;
            }
            settled = true;
            if (error !== undefined && error !== null) {
                reject(error);
                return;
            }
            resolve(undefined);
        };
        stream.on('error', settle);
        stream.on('end', function () { settle(); });
        stream.on('finish', function () { settle(); });
        stream.on('close', function () { settle(); });
    });
    if (typeof callback === 'function') {
        answer.then(function () { callback(null); }, function (error) { callback(error); });
        return undefined;
    }
    return answer;
}

// "pipeline(source, ...transforms, destination)": each stage piped into the next, with the
// whole chain's completion reported once.
function pipeline() {
    var stages = Array.prototype.slice.call(arguments);
    var callback = typeof stages[stages.length - 1] === 'function' ? stages.pop() : null;
    if (stages.length < 2) {
        var error = new TypeError('The "streams" argument must be specified');
        if (callback === null) {
            return Promise.reject(error);
        }
        callback(error);
        return undefined;
    }
    var source = stages[0];
    if (typeof source[Symbol.asyncIterator] === 'function' && typeof source.pipe !== 'function') {
        source = Readable.from(source);
    }
    if (Array.isArray(source)) {
        source = Readable.from(source);
    }
    var last = source;
    for (var stage = 1; stage < stages.length; stage += 1) {
        last = last.pipe(stages[stage]);
    }
    var answer = finished(last);
    if (callback === null) {
        return answer;
    }
    answer.then(function () { callback(null); }, function (failure) { callback(failure); });
    return undefined;
}

exports.Writable = Writable;
exports.Readable = Readable;
exports.Duplex = Duplex;
exports.Transform = Transform;
exports.PassThrough = PassThrough;
exports.Stream = Readable;
exports.pipeline = pipeline;
exports.finished = finished;
exports.promises = { pipeline: pipeline, finished: finished };
"##;

/// `stream/promises`: the promise form of the two whole-stream operations.
pub(super) const STREAM_PROMISES_SOURCE: &str = r##"
'use strict';

module.exports = require('stream').promises;
"##;

/// `string_decoder`: text assembled from byte chunks without splitting a character.
pub(super) const STRING_DECODER_SOURCE: &str = r##"
'use strict';

var Buffer = require('buffer').Buffer;

// How many trailing bytes of a chunk belong to a character the next chunk completes, read
// from the UTF-8 lead byte alone.
function held(bytes) {
    for (var back = 1; back <= 3 && back <= bytes.length; back += 1) {
        var lead = bytes[bytes.length - back];
        if ((lead & 0xc0) !== 0x80) {
            var wanted = lead >= 0xf0 ? 4 : (lead >= 0xe0 ? 3 : (lead >= 0xc0 ? 2 : 1));
            return wanted > back ? back : 0;
        }
    }
    return 0;
}

function StringDecoder(encoding) {
    this.encoding = encoding === undefined || encoding === null ? 'utf8' : String(encoding);
    this.partial = Buffer.alloc(0);
}

StringDecoder.prototype.write = function (chunk) {
    var bytes = Buffer.concat([this.partial, Buffer.from(chunk)]);
    if (this.encoding.toLowerCase().replace(/-/g, '') !== 'utf8') {
        this.partial = Buffer.alloc(0);
        return bytes.toString(this.encoding);
    }
    var keep = held(bytes);
    this.partial = keep === 0 ? Buffer.alloc(0) : bytes.slice(bytes.length - keep);
    return bytes.slice(0, bytes.length - keep).toString('utf8');
};

StringDecoder.prototype.end = function (chunk) {
    var tail = chunk === undefined || chunk === null ? '' : this.write(chunk);
    if (this.partial.length > 0) {
        tail += this.partial.toString(this.encoding);
        this.partial = Buffer.alloc(0);
    }
    return tail;
};

exports.StringDecoder = StringDecoder;
"##;
