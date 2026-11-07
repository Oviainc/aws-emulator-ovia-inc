// SPDX-License-Identifier: CC0-1.0

//! `crypto`: the digests, the keyed digests and the random bytes a Node program asks its
//! runtime for.
//!
//! The arithmetic is NOT written here: a digest and a keyed digest are computed by the host
//! ([`super::super::host_digest`], [`super::super::host_hmac`]) with the RustCrypto
//! implementations this crate already links, and random bytes come from the host's own
//! cryptographic source ([`super::super::host_random_hex`]). This module is the Node-shaped
//! interface over those three: it holds the bytes a program feeds a hash, names the
//! algorithm, and hands the answer back in the encoding the program asked for. Nothing here
//! reaches a host file, socket or environment.

/// `crypto`: hashes, HMACs and random bytes.
pub(super) const CRYPTO_SOURCE: &str = r##"
'use strict';

var Buffer = require('buffer').Buffer;

// The host half of this module: the three pure computations the runtime protocol exposes.
function host() {
    return globalThis.__lambdaRuntime;
}

// The digest algorithms this runtime computes, by the name a Node program names them
// under. One per implementation the host links; an algorithm that is not here is refused
// rather than answered with a digest of another algorithm.
var ALGORITHMS = ['md5', 'sha1', 'sha256', 'sha384', 'sha512'];

// How many bytes one draw of random may ask for, which is the ceiling the HOST enforces
// ([`super::super::digest::MAX_RANDOM_BYTES`]): the draw happens in host memory, outside
// the engine's own allocation ceiling, so the bound is stated on both sides of the call.
var MAX_RANDOM_BYTES = 65536;

function algorithmOf(name) {
    var wanted = String(name).toLowerCase().replace(/-/g, '');
    for (var index = 0; index < ALGORITHMS.length; index += 1) {
        if (ALGORITHMS[index] === wanted) {
            return ALGORITHMS[index];
        }
    }
    var error = new Error('Digest method not supported');
    error.code = 'ERR_CRYPTO_INVALID_DIGEST';
    throw error;
}

function hexOf(data, encoding) {
    if (typeof data === 'string') {
        return Buffer.from(data, encoding === undefined ? 'utf8' : encoding).toString('hex');
    }
    if (data === null || data === undefined) {
        throw new TypeError('The "data" argument must be of type string or an instance of '
            + 'Buffer, TypedArray, or DataView');
    }
    return Buffer.from(data).toString('hex');
}

function encodedDigest(hex, encoding) {
    var bytes = Buffer.from(hex, 'hex');
    return encoding === undefined || encoding === null ? bytes : bytes.toString(encoding);
}

function Hash(algorithm) {
    this.algorithm = algorithmOf(algorithm);
    this.parts = '';
    this.done = false;
}

Hash.prototype.update = function (data, encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.parts += hexOf(data, encoding);
    return this;
};

Hash.prototype.digest = function (encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.done = true;
    return encodedDigest(host().digest(this.algorithm, this.parts), encoding);
};

Hash.prototype.copy = function () {
    var copied = new Hash(this.algorithm);
    copied.parts = this.parts;
    return copied;
};

function Hmac(algorithm, key) {
    this.algorithm = algorithmOf(algorithm);
    this.key = hexOf(key);
    this.parts = '';
    this.done = false;
}

Hmac.prototype.update = function (data, encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.parts += hexOf(data, encoding);
    return this;
};

Hmac.prototype.digest = function (encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.done = true;
    return encodedDigest(host().hmac(this.algorithm, this.key, this.parts), encoding);
};

function randomBytes(size, callback) {
    var count = Math.trunc(Number(size));
    // The size one draw may ask for. Node's own ceiling is `buffer.kMaxLength`, and a
    // request past it is out of range rather than an allocation: the bytes are materialised
    // outside the engine's own heap, so the request is refused before anything is drawn.
    if (!Number.isSafeInteger(count) || count < 0 || count > MAX_RANDOM_BYTES) {
        var error = new RangeError('The value of "size" is out of range. It must be >= 0 && <= '
            + MAX_RANDOM_BYTES + '. Received ' + String(size));
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    var bytes = Buffer.from(host().randomHex(count), 'hex');
    if (typeof callback === 'function') {
        Promise.resolve().then(function () { callback(null, bytes); });
        return undefined;
    }
    return bytes;
}

function randomFillSync(target, offset, size) {
    var at = offset === undefined ? 0 : Math.trunc(Number(offset));
    var count = size === undefined ? target.length - at : Math.trunc(Number(size));
    var bytes = randomBytes(count);
    for (var index = 0; index < count; index += 1) {
        target[at + index] = bytes[index];
    }
    return target;
}

// A v4 UUID: 122 random bits with the version and variant bits the format fixes.
function randomUUID() {
    var bytes = randomBytes(16);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    var hex = bytes.toString('hex');
    return hex.slice(0, 8) + '-' + hex.slice(8, 12) + '-' + hex.slice(12, 16) + '-'
        + hex.slice(16, 20) + '-' + hex.slice(20);
}

// An integer of [min, max), drawn by rejection so every value of the range is equally
// likely however wide the range is.
function randomInt(min, max, callback) {
    var low = max === undefined || typeof max === 'function' ? 0 : Math.trunc(Number(min));
    var high = max === undefined || typeof max === 'function'
        ? Math.trunc(Number(min))
        : Math.trunc(Number(max));
    var done = typeof max === 'function' ? max : callback;
    if (!(high > low)) {
        throw new RangeError('The value of "max" is out of range.');
    }
    var span = high - low;
    var bytes = 0;
    var ceiling = 1;
    while (ceiling < span) {
        ceiling *= 256;
        bytes += 1;
    }
    var drawn = span;
    while (drawn >= ceiling - (ceiling % span)) {
        drawn = 0;
        var held = randomBytes(bytes);
        for (var index = 0; index < bytes; index += 1) {
            drawn = (drawn * 256) + held[index];
        }
    }
    var value = low + (drawn % span);
    if (typeof done === 'function') {
        Promise.resolve().then(function () { done(null, value); });
        return undefined;
    }
    return value;
}

function timingSafeEqual(first, second) {
    var left = Buffer.from(first);
    var right = Buffer.from(second);
    if (left.length !== right.length) {
        throw new RangeError('Input buffers must have the same byte length');
    }
    var difference = 0;
    for (var index = 0; index < left.length; index += 1) {
        difference |= left[index] ^ right[index];
    }
    return difference === 0;
}

function getRandomValues(target) {
    return randomFillSync(target, 0, target.length);
}

var webcrypto = {
    getRandomValues: getRandomValues,
    randomUUID: randomUUID,
    subtle: {
        digest: function (algorithm, data) {
            var named = typeof algorithm === 'string' ? algorithm : algorithm.name;
            var hashed = new Hash(named).update(Buffer.from(data)).digest();
            return Promise.resolve(
                hashed.buffer.slice(hashed.byteOffset, hashed.byteOffset + hashed.length)
            );
        }
    }
};

exports.createHash = function (algorithm) { return new Hash(algorithm); };
exports.createHmac = function (algorithm, key) { return new Hmac(algorithm, key); };
exports.Hash = Hash;
exports.Hmac = Hmac;
exports.getHashes = function () { return ALGORITHMS.slice(); };
exports.randomBytes = randomBytes;
exports.pseudoRandomBytes = randomBytes;
exports.randomFillSync = randomFillSync;
exports.randomFill = function (target, offset, size, callback) {
    var done = typeof offset === 'function' ? offset
        : (typeof size === 'function' ? size : callback);
    var at = typeof offset === 'function' ? 0 : offset;
    var count = typeof size === 'function' || size === undefined ? undefined : size;
    var filled = randomFillSync(target, at, count);
    Promise.resolve().then(function () { done(null, filled); });
};
exports.randomUUID = randomUUID;
exports.randomInt = randomInt;
exports.timingSafeEqual = timingSafeEqual;
exports.getRandomValues = getRandomValues;
exports.webcrypto = webcrypto;
exports.subtle = webcrypto.subtle;
exports.constants = {};
"##;
