// SPDX-License-Identifier: CC0-1.0

//! `Intl` and the LOCAL TIME ZONE a run reads its clock in.
//!
//! A managed Node runtime defines `Intl` and honours `TZ`: a handler reads the local time
//! of the zone its configuration names, and formats dates and numbers through `Intl`. The
//! engine here has neither — it has no locale data and no zone database of its own, and its
//! `Date` reads whatever zone the host process is in, which is not the function's.
//!
//! So both are served from facts this emulator holds. The zone offsets are the IANA
//! database's, looked up by the host ([`super::zone_offset_ms`], which is `chrono-tz`), so a
//! named zone's offset — including its daylight-saving transitions — is that database's
//! answer and not a guess. The LOCALE data is the one locale this runtime provides, `en-US`,
//! which is what `resolvedOptions().locale` reports for every request: a Node build carrying
//! one locale resolves every locale to it rather than formatting another locale's
//! conventions it does not have.

/// The source installing `Intl` and the local-zone reading of `Date`, evaluated once per
/// invocation after the runtime's own source.
pub(super) const INTL_SOURCE: &str = r##"
'use strict';
(function () {
    var host = globalThis.__lambdaRuntime;

    // The one locale this runtime carries data for, which every request resolves to.
    var LOCALE = 'en-US';

    var MONTHS = [
        'January', 'February', 'March', 'April', 'May', 'June',
        'July', 'August', 'September', 'October', 'November', 'December'
    ];
    var DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

    // The zone a run reads its local clock in: what `TZ` names, in the form a managed
    // runtime sets it (`:UTC` is the zone `UTC`), and `UTC` when it names none.
    function defaultZone() {
        var named = globalThis.process === undefined ? undefined : globalThis.process.env.TZ;
        if (typeof named !== 'string' || named === '') {
            return 'UTC';
        }
        return named.charAt(0) === ':' ? named.slice(1) : named;
    }

    // A zone written as a fixed offset from UTC, which `Intl` accepts beside a named zone.
    var FIXED_OFFSET = /^(?:GMT|UTC)?([+-])([0-9]{1,2})(?::?([0-9]{2}))?$/;

    // The offset of `zone` at the instant `time`, in milliseconds, or `null` when no zone
    // of that name exists. A named zone's offset is the IANA database's answer for that
    // instant, so a zone's daylight-saving rules are applied rather than assumed away.
    function zoneOffsetMs(zone, time) {
        if (zone === 'UTC' || zone === 'Etc/UTC' || zone === 'GMT' || zone === 'Z') {
            return 0;
        }
        var fixed = FIXED_OFFSET.exec(zone);
        if (fixed !== null) {
            var minutes = (Number(fixed[2]) * 60) + Number(fixed[3] === undefined ? 0 : fixed[3]);
            return (fixed[1] === '-' ? -minutes : minutes) * 60000;
        }
        return host.zoneOffsetMs(zone, time);
    }

    function invalidZone(zone) {
        var error = new RangeError('Invalid time zone specified: ' + zone);
        return error;
    }

    // The calendar fields of `time` in `zone`, read off the UTC fields of the instant the
    // offset shifts it to — which is what reading a clock in that zone is.
    function fieldsOf(time, zone) {
        var offset = zoneOffsetMs(zone, time);
        if (offset === null || offset === undefined) {
            throw invalidZone(zone);
        }
        var shifted = new Date(time + offset);
        return {
            year: shifted.getUTCFullYear(),
            month: shifted.getUTCMonth(),
            day: shifted.getUTCDate(),
            weekday: shifted.getUTCDay(),
            hour: shifted.getUTCHours(),
            minute: shifted.getUTCMinutes(),
            second: shifted.getUTCSeconds(),
            millisecond: shifted.getUTCMilliseconds(),
            offsetMinutes: offset / 60000
        };
    }

    function padded(value, width) {
        var text = String(Math.abs(value));
        while (text.length < width) {
            text = '0' + text;
        }
        return value < 0 ? '-' + text : text;
    }

    function hour12(hour) {
        var held = hour % 12;
        return held === 0 ? 12 : held;
    }

    function offsetName(minutes) {
        var sign = minutes < 0 ? '-' : '+';
        var held = Math.abs(minutes);
        return 'GMT' + sign + padded(Math.floor(held / 60), 1)
            + (held % 60 === 0 ? '' : ':' + padded(held % 60, 2));
    }

    // Which fields a format carries. With no option at all a date format carries the three
    // numeric date fields, which is what `Intl.DateTimeFormat` resolves an empty request to.
    function resolvedFields(options) {
        var held = options === undefined || options === null ? {} : options;
        var named = ['weekday', 'era', 'year', 'month', 'day', 'hour', 'minute', 'second',
            'timeZoneName'];
        var wanted = {};
        var any = false;
        for (var index = 0; index < named.length; index += 1) {
            if (held[named[index]] !== undefined) {
                wanted[named[index]] = held[named[index]];
                any = true;
            }
        }
        if (held.dateStyle !== undefined) {
            wanted.year = held.dateStyle === 'short' ? '2-digit' : 'numeric';
            wanted.month = held.dateStyle === 'long' || held.dateStyle === 'full'
                ? 'long'
                : 'numeric';
            wanted.day = 'numeric';
            if (held.dateStyle === 'full') {
                wanted.weekday = 'long';
            }
            any = true;
        }
        if (held.timeStyle !== undefined) {
            wanted.hour = 'numeric';
            wanted.minute = '2-digit';
            if (held.timeStyle !== 'short') {
                wanted.second = '2-digit';
            }
            any = true;
        }
        if (!any) {
            wanted.year = 'numeric';
            wanted.month = 'numeric';
            wanted.day = 'numeric';
        }
        return wanted;
    }

    function DateTimeFormat(locales, options) {
        if (!(this instanceof DateTimeFormat)) {
            return new DateTimeFormat(locales, options);
        }
        var held = options === undefined || options === null ? {} : options;
        var zone = held.timeZone === undefined || held.timeZone === null
            ? defaultZone()
            : String(held.timeZone);
        // A zone with no entry in the database is not a zone, which is what `Intl` reports
        // for one: the check happens here, where the format is built, as it does there.
        if (zoneOffsetMs(zone, Date.now()) === null) {
            throw invalidZone(zone);
        }
        this._zone = zone;
        this._fields = resolvedFields(held);
        this._hour12 = held.hour12 === undefined
            ? (held.hourCycle === undefined ? true : held.hourCycle === 'h11' || held.hourCycle === 'h12')
            : held.hour12 === true;
        this._requested = locales;
    }

    // The parts of `date` in this format's zone, in the order the `en-US` conventions put
    // them: the date fields as month/day/year, then the time fields.
    DateTimeFormat.prototype.formatToParts = function (date) {
        var time = date === undefined ? Date.now()
            : (date instanceof Date ? date.getTime() : Number(date));
        var fields = fieldsOf(time, this._zone);
        var wanted = this._fields;
        var parts = [];
        var push = function (type, value) { parts.push({ type: type, value: value }); };
        if (wanted.weekday !== undefined) {
            push('weekday', wanted.weekday === 'short'
                ? DAYS[fields.weekday].slice(0, 3)
                : DAYS[fields.weekday]);
        }
        var dated = wanted.year !== undefined || wanted.month !== undefined
            || wanted.day !== undefined;
        if (dated) {
            if (parts.length > 0) {
                push('literal', ', ');
            }
            var textualMonth = wanted.month === 'long' || wanted.month === 'short';
            if (textualMonth) {
                push('month', wanted.month === 'short'
                    ? MONTHS[fields.month].slice(0, 3)
                    : MONTHS[fields.month]);
                if (wanted.day !== undefined) {
                    push('literal', ' ');
                    push('day', String(fields.day));
                }
                if (wanted.year !== undefined) {
                    push('literal', ', ');
                }
            } else {
                if (wanted.month !== undefined) {
                    push('month', wanted.month === '2-digit'
                        ? padded(fields.month + 1, 2)
                        : String(fields.month + 1));
                }
                if (wanted.day !== undefined) {
                    if (wanted.month !== undefined) {
                        push('literal', '/');
                    }
                    push('day', wanted.day === '2-digit'
                        ? padded(fields.day, 2)
                        : String(fields.day));
                }
                if (wanted.year !== undefined && (wanted.month !== undefined
                    || wanted.day !== undefined)) {
                    push('literal', '/');
                }
            }
            if (wanted.year !== undefined) {
                push('year', wanted.year === '2-digit'
                    ? padded(fields.year % 100, 2)
                    : String(fields.year));
            }
        }
        var timed = wanted.hour !== undefined || wanted.minute !== undefined
            || wanted.second !== undefined;
        if (timed) {
            if (parts.length > 0) {
                push('literal', ', ');
            }
            if (wanted.hour !== undefined) {
                var shown = this._hour12 ? hour12(fields.hour) : fields.hour;
                push('hour', wanted.hour === '2-digit' && !this._hour12
                    ? padded(shown, 2)
                    : String(shown));
            }
            if (wanted.minute !== undefined) {
                if (wanted.hour !== undefined) {
                    push('literal', ':');
                }
                push('minute', padded(fields.minute, 2));
            }
            if (wanted.second !== undefined) {
                if (wanted.hour !== undefined || wanted.minute !== undefined) {
                    push('literal', ':');
                }
                push('second', padded(fields.second, 2));
            }
            if (this._hour12 && wanted.hour !== undefined) {
                push('literal', ' ');
                push('dayPeriod', fields.hour < 12 ? 'AM' : 'PM');
            }
        }
        if (wanted.timeZoneName !== undefined) {
            push('literal', ' ');
            push('timeZoneName', offsetName(fields.offsetMinutes));
        }
        return parts;
    };

    DateTimeFormat.prototype.format = function (date) {
        return this.formatToParts(date)
            .map(function (part) { return part.value; })
            .join('');
    };

    DateTimeFormat.prototype.resolvedOptions = function () {
        var resolved = {
            locale: LOCALE,
            calendar: 'gregory',
            numberingSystem: 'latn',
            timeZone: this._zone
        };
        var names = Object.keys(this._fields);
        for (var index = 0; index < names.length; index += 1) {
            resolved[names[index]] = this._fields[names[index]];
        }
        if (this._fields.hour !== undefined) {
            resolved.hour12 = this._hour12;
            resolved.hourCycle = this._hour12 ? 'h12' : 'h23';
        }
        return resolved;
    };

    DateTimeFormat.prototype.formatRange = function (from, to) {
        return this.format(from) + ' – ' + this.format(to);
    };

    // Every request resolves to the one locale this runtime carries, so the supported set
    // of any request is that locale when it was asked for and empty otherwise.
    function supportedLocalesOf(locales) {
        var asked = locales === undefined ? [] : (Array.isArray(locales) ? locales : [locales]);
        return asked.filter(function (tag) {
            return String(tag).toLowerCase().indexOf('en') === 0;
        });
    }

    DateTimeFormat.supportedLocalesOf = supportedLocalesOf;

    function NumberFormat(locales, options) {
        if (!(this instanceof NumberFormat)) {
            return new NumberFormat(locales, options);
        }
        var held = options === undefined || options === null ? {} : options;
        this._minimumFractionDigits = held.minimumFractionDigits === undefined
            ? 0
            : Number(held.minimumFractionDigits);
        this._maximumFractionDigits = held.maximumFractionDigits === undefined
            ? Math.max(3, this._minimumFractionDigits)
            : Number(held.maximumFractionDigits);
        this._useGrouping = held.useGrouping !== false;
        this._style = held.style === undefined ? 'decimal' : held.style;
        this._currency = held.currency;
    }

    NumberFormat.prototype.format = function (value) {
        var number = Number(value);
        if (!isFinite(number)) {
            return String(number);
        }
        var scaled = this._style === 'percent' ? number * 100 : number;
        var text = Math.abs(scaled).toFixed(this._maximumFractionDigits);
        var split = text.split('.');
        var fraction = split.length > 1 ? split[1].replace(/0+$/, '') : '';
        while (fraction.length < this._minimumFractionDigits) {
            fraction += '0';
        }
        var whole = split[0];
        if (this._useGrouping) {
            // The `en-US` grouping: three digits per group, separated by a comma.
            whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
        }
        var body = whole + (fraction === '' ? '' : '.' + fraction);
        var signed = (scaled < 0 ? '-' : '') + body;
        if (this._style === 'percent') {
            return signed + '%';
        }
        if (this._style === 'currency' && this._currency === 'USD') {
            return (scaled < 0 ? '-$' : '$') + body;
        }
        return signed;
    };

    NumberFormat.prototype.resolvedOptions = function () {
        return {
            locale: LOCALE,
            numberingSystem: 'latn',
            style: this._style,
            currency: this._currency,
            minimumIntegerDigits: 1,
            minimumFractionDigits: this._minimumFractionDigits,
            maximumFractionDigits: this._maximumFractionDigits,
            useGrouping: this._useGrouping
        };
    };

    NumberFormat.supportedLocalesOf = supportedLocalesOf;

    function Collator(locales, options) {
        if (!(this instanceof Collator)) {
            return new Collator(locales, options);
        }
        var held = options === undefined || options === null ? {} : options;
        this._numeric = held.numeric === true;
        this._caseless = held.sensitivity === 'base' || held.sensitivity === 'accent';
    }

    Collator.prototype.compare = function (first, second) {
        var left = this._caseless ? String(first).toLowerCase() : String(first);
        var right = this._caseless ? String(second).toLowerCase() : String(second);
        if (this._numeric) {
            var leftNumber = Number(left);
            var rightNumber = Number(right);
            if (isFinite(leftNumber) && isFinite(rightNumber) && leftNumber !== rightNumber) {
                return leftNumber < rightNumber ? -1 : 1;
            }
        }
        if (left === right) {
            return 0;
        }
        return left < right ? -1 : 1;
    };

    Collator.prototype.resolvedOptions = function () {
        return {
            locale: LOCALE,
            usage: 'sort',
            sensitivity: this._caseless ? 'base' : 'variant',
            numeric: this._numeric
        };
    };

    Collator.supportedLocalesOf = supportedLocalesOf;

    globalThis.Intl = {
        DateTimeFormat: DateTimeFormat,
        NumberFormat: NumberFormat,
        Collator: Collator,
        getCanonicalLocales: function (locales) {
            var asked = locales === undefined ? [] : (Array.isArray(locales) ? locales : [locales]);
            return asked.map(String);
        }
    };

    // The LOCAL clock a run reads. The engine's own local time is the host process's, which
    // is not the function's: every reading below is the zone `TZ` names, so a handler that
    // reads a local field sees the zone its configuration sets.
    var local = function (time) {
        var offset = zoneOffsetMs(defaultZone(), time);
        return new Date(time + (offset === null ? 0 : offset));
    };
    var readings = [
        ['getFullYear', 'getUTCFullYear'],
        ['getMonth', 'getUTCMonth'],
        ['getDate', 'getUTCDate'],
        ['getDay', 'getUTCDay'],
        ['getHours', 'getUTCHours'],
        ['getMinutes', 'getUTCMinutes'],
        ['getSeconds', 'getUTCSeconds'],
        ['getMilliseconds', 'getUTCMilliseconds']
    ];
    for (var reading = 0; reading < readings.length; reading += 1) {
        (function (pair) {
            Date.prototype[pair[0]] = function () {
                return local(this.getTime())[pair[1]]();
            };
        })(readings[reading]);
    }
    Date.prototype.getTimezoneOffset = function () {
        var offset = zoneOffsetMs(defaultZone(), this.getTime());
        return -(offset === null ? 0 : offset) / 60000;
    };
    // The STAMPED readings, re-based the same way: a date, a time and the two together are
    // read in the zone `TZ` names, with that zone's offset at that instant and the zone's
    // own name — which is what the reading names a zone by here, the display names of the
    // locale data a managed runtime carries not being part of the zone database.
    function offsetStamp(minutes) {
        var sign = minutes < 0 ? '-' : '+';
        var held = Math.abs(minutes);
        return 'GMT' + sign + padded(Math.floor(held / 60), 2) + padded(held % 60, 2);
    }
    Date.prototype.toDateString = function () {
        if (Number.isNaN(this.getTime())) {
            return 'Invalid Date';
        }
        var fields = fieldsOf(this.getTime(), defaultZone());
        return DAYS[fields.weekday].slice(0, 3) + ' ' + MONTHS[fields.month].slice(0, 3)
            + ' ' + padded(fields.day, 2) + ' ' + padded(fields.year, 4);
    };
    Date.prototype.toTimeString = function () {
        if (Number.isNaN(this.getTime())) {
            return 'Invalid Date';
        }
        var fields = fieldsOf(this.getTime(), defaultZone());
        return padded(fields.hour, 2) + ':' + padded(fields.minute, 2) + ':'
            + padded(fields.second, 2) + ' ' + offsetStamp(fields.offsetMinutes)
            + ' (' + defaultZone() + ')';
    };
    Date.prototype.toString = function () {
        if (Number.isNaN(this.getTime())) {
            return 'Invalid Date';
        }
        return this.toDateString() + ' ' + this.toTimeString();
    };
    Date.prototype.toLocaleDateString = function (locales, options) {
        return new DateTimeFormat(locales, options === undefined
            ? { year: 'numeric', month: 'numeric', day: 'numeric' }
            : options).format(this);
    };
    Date.prototype.toLocaleTimeString = function (locales, options) {
        return new DateTimeFormat(locales, options === undefined
            ? { hour: 'numeric', minute: '2-digit', second: '2-digit' }
            : options).format(this);
    };
    Date.prototype.toLocaleString = function (locales, options) {
        return new DateTimeFormat(locales, options === undefined
            ? {
                year: 'numeric', month: 'numeric', day: 'numeric',
                hour: 'numeric', minute: '2-digit', second: '2-digit'
            }
            : options).format(this);
    };
})();
"##;
