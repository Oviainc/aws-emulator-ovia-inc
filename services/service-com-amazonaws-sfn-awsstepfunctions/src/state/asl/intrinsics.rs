// SPDX-License-Identifier: CC0-1.0

//! The Amazon States Language's INTRINSIC FUNCTIONS: the parser for one invocation's
//! own syntax (`States.Format('{}', $.name)`), and what each named function computes.
//!
//! One concern of the interpreter, held in one file: the invocation text lives inside a
//! single JSON string of a caller's definition, so what it may ask for is bounded here
//! ([`MAX_EXPRESSION_CHARS`], [`MAX_EXPRESSION_DEPTH`], [`MAX_ARRAY_ITEMS`]) rather
//! than by the document around it.

use super::*;

/// Evaluate an intrinsic function invocation (`States.Format('{}', $.name)`).
pub(super) fn intrinsic(text: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    invoke_intrinsic(text, input, context, 1)
}

/// [`intrinsic`], carrying how deeply the invocation nests.
///
/// This is this crate's own parser for ASL's intrinsic syntax, over text that is a
/// value of a definition the model bounds only at 1 MiB — and the nesting lives
/// inside ONE JSON string, so `serde_json`'s own recursion limit does not bound it.
/// Both the length of the invocation ([`MAX_EXPRESSION_CHARS`]) and how deeply it
/// nests ([`MAX_EXPRESSION_DEPTH`]) are therefore bounded here, and an invocation
/// past either is reported with [`RUNTIME_ERROR`] rather than parsed.
pub(super) fn invoke_intrinsic(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(runtime(format!(
            "an intrinsic-function invocation nests more than the {MAX_EXPRESSION_DEPTH} \
             levels this service evaluates"
        )));
    }
    if text.len() > MAX_EXPRESSION_CHARS {
        return Err(runtime(format!(
            "an intrinsic-function invocation of {} bytes is longer than the \
             {MAX_EXPRESSION_CHARS} this service evaluates",
            text.len()
        )));
    }
    let open = text
        .find('(')
        .ok_or_else(|| runtime(format!("'{text}' is not an intrinsic-function invocation")))?;
    if !text.ends_with(')') {
        return Err(runtime(format!(
            "the intrinsic invocation '{text}' is not closed"
        )));
    }
    let name = text[..open].trim();
    let arguments = split_arguments(&text[open + 1..text.len() - 1])
        .map_err(|why| runtime(format!("'{text}': {why}")))?;
    // Each argument may select a whole document, and an invocation may carry as many
    // arguments as its text has room for, so the arguments are measured as they are
    // evaluated and bounded together: the function below cannot be handed more than a
    // state's output may carry, so it cannot build more either.
    let mut spent = 0usize;
    let mut values = Vec::with_capacity(arguments.len());
    for argument in &arguments {
        let value = argument_value(argument, input, context, depth)?;
        spent = spent.saturating_add(measure_payload(&value, "an intrinsic argument")?);
        if spent > MAX_PAYLOAD_BYTES {
            return Err(runtime(format!(
                "'{name}' was given at least {spent} bytes of arguments, more than the \
                 {MAX_PAYLOAD_BYTES} a state's output may carry"
            )));
        }
        values.push(value);
    }
    let result = apply_intrinsic(name, &values, &arguments)
        .map_err(|why| runtime(format!("'{text}': {why}")))?;
    // And what it built is bounded too, because a function may expand what it was
    // given.
    measure_payload(&result, "an intrinsic function")?;
    Ok(result)
}

/// Split an intrinsic's argument list on the commas that are not inside a nested
/// invocation or a single-quoted string.
pub(super) fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' if quoted => {
                current.push(character);
                if let Some(escaped) = characters.next() {
                    current.push(escaped);
                }
            }
            '\'' => {
                quoted = !quoted;
                current.push(character);
            }
            '(' if !quoted => {
                depth += 1;
                current.push(character);
            }
            ')' if !quoted => {
                depth = depth.checked_sub(1).ok_or("unbalanced ')'")?;
                current.push(character);
            }
            ',' if !quoted && depth == 0 => {
                arguments.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if quoted {
        return Err("an unterminated quoted string".to_string());
    }
    if depth != 0 {
        return Err("an unclosed '('".to_string());
    }
    if !current.trim().is_empty() || !arguments.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

/// One intrinsic argument: a nested invocation, a path, a quoted string, or a JSON
/// literal. `depth` is the nesting of the invocation this argument belongs to.
pub(super) fn argument_value(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if text.starts_with("States.") && text.contains('(') {
        return invoke_intrinsic(text, input, context, depth + 1);
    }
    if text.starts_with('$') {
        return select(text, input, context);
    }
    if let Some(quoted) = quoted_literal(text) {
        // "escape ... the following characters: ' { } \" with a backslash.
        let mut unescaped = String::with_capacity(quoted.len());
        let mut characters = quoted.chars();
        while let Some(character) = characters.next() {
            if character == '\\' {
                if let Some(escaped) = characters.next() {
                    unescaped.push(escaped);
                }
            } else {
                unescaped.push(character);
            }
        }
        return Ok(Value::String(unescaped));
    }
    serde_json::from_str(text).map_err(|error| {
        runtime(format!(
            "'{text}' is not a path, a quoted string or a JSON literal: {error}"
        ))
    })
}

/// The inside of an intrinsic argument written as a single-quoted string LITERAL,
/// with its backslash escapes still in place; `None` for an argument that is a path,
/// a nested invocation or a JSON literal.
pub(super) fn quoted_literal(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
}

/// The hashing algorithms `States.Hash` names — "MD5, SHA-1, SHA-256, SHA-384 and
/// SHA-512" — each computed by the maintained implementation of it.
///
/// The list is the function's own: an algorithm is in it because the language's
/// `States.Hash` documents it, so a definition asking for one gets the digest the
/// real service would return. The digest is a DATA TRANSFORMATION a payload template
/// asked for and nothing else — nothing in this emulator decides anything on one, so
/// MD5 and SHA-1 are here as members of a published catalog rather than as a choice
/// of cryptography.
pub(super) const HASH_ALGORITHMS: &[&str] = &["MD5", "SHA-1", "SHA-256", "SHA-384", "SHA-512"];

/// `data` hashed with one of [`HASH_ALGORITHMS`], as the lowercase hex digest
/// `States.Hash` returns; `None` for a name that is not one of them.
fn hash_hex(algorithm: &str, data: &[u8]) -> Option<String> {
    // One `Digest` trait, implemented by each of the three crates below.
    use ::sha2::Digest as _;
    Some(match algorithm {
        "MD5" => ::hex::encode(::md5::Md5::digest(data)),
        "SHA-1" => ::hex::encode(::sha1::Sha1::digest(data)),
        "SHA-256" => ::hex::encode(::sha2::Sha256::digest(data)),
        "SHA-384" => ::hex::encode(::sha2::Sha384::digest(data)),
        "SHA-512" => ::hex::encode(::sha2::Sha512::digest(data)),
        _ => return None,
    })
}

/// Apply one intrinsic function to its evaluated arguments.
///
/// `raw` carries each argument's own TEXT, as the invocation wrote it, because the
/// backslash escaping of `'`, `{`, `}` and `\` is syntax of that text rather than of
/// the value it denotes: `States.Format` therefore scans its template from the
/// literal the invocation carries, where a brace an escape made literal is data and
/// consumes no argument.
///
/// The set below is every documented intrinsic. Like any other name this closed
/// allow-list does not carry, one outside it is reported with [`RUNTIME_ERROR`]
/// rather than approximated.
pub(super) fn apply_intrinsic(
    name: &str,
    arguments: &[Value],
    raw: &[String],
) -> Result<Value, String> {
    let argument = |index: usize| -> Result<&Value, String> {
        arguments
            .get(index)
            .ok_or_else(|| format!("'{name}' needs at least {} arguments", index + 1))
    };
    // A JSON number is finite by construction, but the arithmetic below is only
    // total for a finite value, so the guard is stated rather than assumed.
    let number = |index: usize| -> Result<f64, String> {
        argument(index)?
            .as_f64()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("argument {} of '{name}' must be a finite number", index + 1))
    };
    let string = |index: usize| -> Result<String, String> {
        argument(index)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("argument {} of '{name}' must be a string", index + 1))
    };
    let array = |index: usize| -> Result<Vec<Value>, String> {
        let items = argument(index)?
            .as_array()
            .cloned()
            .ok_or_else(|| format!("argument {} of '{name}' must be an array", index + 1))?;
        if items.len() > MAX_ARRAY_ITEMS {
            return Err(format!(
                "argument {} of '{name}' carries {} items, more than the {MAX_ARRAY_ITEMS} this \
                 function walks",
                index + 1,
                items.len()
            ));
        }
        Ok(items)
    };

    match name {
        "States.Format" => {
            // A brace pair is a placeholder only where the invocation wrote it BARE:
            // inside a quoted literal a backslash makes the character after it data,
            // so `\{`, `\}`, `\'` and `\\` stand for themselves and consume no
            // argument. The escapes are therefore applied HERE, over the literal's
            // own text, and not a second time over the value it denotes. A template
            // that came from a path or a nested invocation is not invocation text, so
            // a backslash in it is data of its own.
            let selected = string(0)?;
            let literal = raw.first().and_then(|text| quoted_literal(text));
            let template = literal.unwrap_or(selected.as_str());
            let mut rendered = String::with_capacity(template.len());
            let mut next = 1usize;
            let mut characters = template.chars().peekable();
            while let Some(character) = characters.next() {
                match character {
                    '\\' if literal.is_some() => {
                        if let Some(escaped) = characters.next() {
                            rendered.push(escaped);
                        }
                    }
                    '{' if characters.peek() == Some(&'}') => {
                        characters.next();
                        let value = argument(next)?;
                        next += 1;
                        match value {
                            Value::String(text) => rendered.push_str(text),
                            other => rendered.push_str(&other.to_string()),
                        }
                    }
                    other => rendered.push(other),
                }
            }
            Ok(Value::String(rendered))
        }
        "States.StringToJson" => {
            let text = string(0)?;
            serde_json::from_str(&text).map_err(|error| format!("{text:?} is not JSON: {error}"))
        }
        "States.JsonToString" => Ok(Value::String(argument(0)?.to_string())),
        "States.Array" => Ok(Value::Array(arguments.to_vec())),
        "States.ArrayPartition" => {
            let items = array(0)?;
            let size = number(1)?;
            if size < 1.0 {
                return Err("the chunk size must be a positive whole number".to_string());
            }
            Ok(Value::Array(
                items
                    .chunks(size as usize)
                    .map(|chunk| Value::Array(chunk.to_vec()))
                    .collect(),
            ))
        }
        "States.ArrayContains" => {
            let items = array(0)?;
            let looked_for = argument(1)?;
            Ok(Value::Bool(items.iter().any(|item| item == looked_for)))
        }
        "States.ArrayRange" => {
            let (start, end, step) = (number(0)?, number(1)?, number(2)?);
            if step == 0.0 {
                return Err("the step must not be zero".to_string());
            }
            let mut values = Vec::new();
            let mut current = start;
            while (step > 0.0 && current <= end) || (step < 0.0 && current >= end) {
                values.push(json_number(current));
                current += step;
                // The array is built from three numbers, so nothing the caller sent
                // bounds its length: this does.
                if values.len() > MAX_ARRAY_RANGE_VALUES {
                    return Err(format!(
                        "the range is longer than the {MAX_ARRAY_RANGE_VALUES} values this \
                         function builds"
                    ));
                }
            }
            Ok(Value::Array(values))
        }
        "States.ArrayGetItem" => {
            let items = array(0)?;
            let index = number(1)?;
            if index < 0.0 {
                return Err("the index must not be negative".to_string());
            }
            items
                .get(index as usize)
                .cloned()
                .ok_or_else(|| format!("index {index} is past the end of the array"))
        }
        "States.ArrayLength" => Ok(json!(array(0)?.len())),
        "States.ArrayUnique" => {
            // Compared by the canonical TEXT of each value, through a set, so the
            // pass is O(n log n) rather than the O(n^2) a linear scan per item would
            // cost on an array the execution input sized.
            let mut seen = BTreeSet::new();
            let mut unique: Vec<Value> = Vec::new();
            for item in array(0)? {
                if seen.insert(item.to_string()) {
                    unique.push(item);
                }
            }
            Ok(Value::Array(unique))
        }
        "States.JsonMerge" => {
            let mut merged = argument(0)?
                .as_object()
                .cloned()
                .ok_or_else(|| format!("argument 1 of '{name}' must be a JSON object"))?;
            let second = argument(1)?
                .as_object()
                .ok_or_else(|| format!("argument 2 of '{name}' must be a JSON object"))?;
            let deep = argument(2)?.as_bool().unwrap_or(false);
            if deep {
                // "Currently, Step Functions only supports the shallow merging mode;
                // therefore, you must specify the Boolean value as false."
                return Err(format!(
                    "'{name}' performs a shallow merge, so argument 3 must be false"
                ));
            }
            for (key, value) in second {
                merged.insert(key.clone(), value.clone());
            }
            Ok(Value::Object(merged))
        }
        "States.MathAdd" => Ok(json_number(number(0)? + number(1)?)),
        "States.MathRandom" => {
            let (start, end) = (number(0)?, number(1)?);
            if end < start {
                return Err("the end of the range must not be below its start".to_string());
            }
            let mut seed = match arguments.get(2).and_then(Value::as_f64) {
                Some(seed) => seed as u64,
                None => super::super::now_nanos(),
            };
            // The draw is in `[start, end)`: the end of the range is EXCLUSIVE, so
            // the span is the DISTANCE between the bounds rather than one more than
            // it, and a range whose bounds meet carries only its start.
            //
            // Both bounds are caller-chosen finite numbers, so every step of this is
            // saturating: a float-to-integer cast saturates, and the addition would
            // otherwise overflow for a range near the integer maxima.
            let span = (end - start) as u64;
            let drawn = super::super::splitmix64(&mut seed) % span.max(1);
            let drawn = i64::try_from(drawn).unwrap_or(i64::MAX);
            Ok(json!((start as i64).saturating_add(drawn)))
        }
        // "States.Base64Encode ... encode data based on MIME Base64 encoding scheme.
        // You can use this function to pass data to other AWS services without using
        // an AWS Lambda function", with a documented 10,000-character input limit.
        "States.Base64Encode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     encodes",
                    text.len()
                ));
            }
            Ok(Value::String(::aws_smithy_types::base64::encode(text)))
        }
        "States.Base64Decode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     decodes",
                    text.len()
                ));
            }
            let decoded = ::aws_smithy_types::base64::decode(&text)
                .map_err(|error| format!("the input is not MIME Base64: {error}"))?;
            String::from_utf8(decoded)
                .map(Value::String)
                .map_err(|error| format!("the decoded data is not UTF-8 text: {error}"))
        }
        "States.StringSplit" => {
            let text = string(0)?;
            let delimiters: Vec<char> = string(1)?.chars().collect();
            let parts = text
                .split(|character| delimiters.contains(&character))
                .filter(|part| !part.is_empty())
                .map(|part| Value::String(part.to_string()))
                .collect();
            Ok(Value::Array(parts))
        }
        // "States.Hash(data, algorithm) ... calculate the hash value of a given
        // input", returned as the hex digest of one of the algorithms the language
        // names, with a documented 10,000-character input limit.
        "States.Hash" => {
            let data = string(0)?;
            let characters = data.chars().count();
            if characters > MAX_HASH_CHARS {
                return Err(format!(
                    "the input is {characters} characters, more than the {MAX_HASH_CHARS} this \
                     function hashes"
                ));
            }
            let algorithm = string(1)?;
            hash_hex(&algorithm, data.as_bytes())
                .map(Value::String)
                .ok_or_else(|| {
                    format!(
                        "argument 2 of '{name}' must be one of {}, not {algorithm:?}",
                        HASH_ALGORITHMS.join(", ")
                    )
                })
        }
        "States.UUID" => {
            let mut seed = super::super::now_nanos();
            let high = super::super::splitmix64(&mut seed);
            let low = super::super::splitmix64(&mut seed);
            Ok(Value::String(super::super::uuid_v4(high, low)))
        }
        other => Err(format!(
            "'{other}' is an intrinsic function this emulator does not evaluate, so nothing was \
             run for it"
        )),
    }
}

/// A number an intrinsic computed, as the JSON value the wire carries: a whole
/// result is a whole number, which is what the service's own intrinsics return.
pub(super) fn json_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return json!(value as i64);
    }
    json!(value)
}
