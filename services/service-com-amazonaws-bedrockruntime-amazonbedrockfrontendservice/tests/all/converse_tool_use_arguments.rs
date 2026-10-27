// SPDX-License-Identifier: CC0-1.0

//! The ARGUMENTS a forced tool call carries, end to end over HTTP: the `input`
//! document of the `toolUse` block `Converse` answers with, and the
//! `contentBlockDelta` fragments `ConverseStream` frames the same document as.
//!
//! RULE under test: a tool is called with the argument object its own
//! `toolSpec.inputSchema.json` describes, so every name that schema REQUIRES is
//! a key of `toolUse.input` carrying a value of the type declared for it (and
//! where the schema requires nothing, every property it declares is a key), and
//! the streamed fragments concatenate to exactly that document.
//!
//! Every case drives the real router through the harness the other suites of
//! this target use, so what is asserted is the wire behaviour a client sees. The
//! table spans what the rule generalises over: how many names the schema
//! requires, which JSON types it declares for them, whether the required names
//! are all of its properties or a subset, whether it fixes a value itself
//! (`const`, `enum`, `default`), the bounds it declares, nesting (an object
//! inside an object, arrays, a local `$ref`), and a schema that declares nothing
//! at all.

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;

use crate::bedrock_runtime_inference::{emulator, post_json};
use crate::bedrock_runtime_streaming::decode_frames;

/// A model the registry serves and the Converse API supports.
const MODEL_ID: &str = "amazon.nova-lite-v1:0";

/// One tool schema, and the argument document a request for that tool must
/// carry.
struct SchemaRow {
    case: &'static str,
    /// The `toolSpec.inputSchema.json` the tool is declared with.
    schema: Value,
    /// The whole `toolUse.input` document the request must answer with.
    arguments: Value,
}

/// The schema forms a tool can be declared with, one row each.
///
/// The JSON Schema keywords exercised here are the ones a tool schema is written
/// with (`type`, `properties`, `required`, `enum`, `const`, `default`,
/// `minLength`/`maxLength`, `minimum`/`maximum`, `items`/`minItems`,
/// `additionalProperties`, `anyOf`, `$ref`/`$defs` — JSON Schema Reference, the
/// document `ToolInputSchema.json` points at); the member names of the tool
/// surface come from `smithy_query(query="shape",
/// shape="ToolSpecification,ToolInputSchema,ToolUseBlock,ToolUseBlockDelta")` on
/// this service's model.
fn schema_rows() -> Vec<SchemaRow> {
    vec![
        SchemaRow {
            case: "one required string property",
            schema: json!({
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"],
            }),
            arguments: json!({"location": "location"}),
        },
        SchemaRow {
            case: "two required string properties",
            schema: json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"},
                    "description": {"type": "string"},
                },
                "required": ["city", "description"],
            }),
            arguments: json!({"city": "city", "description": "description"}),
        },
        SchemaRow {
            case: "one required property of each scalar type",
            schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "count": {"type": "integer"},
                    "ratio": {"type": "number"},
                    "urgent": {"type": "boolean"},
                },
                "required": ["name", "count", "ratio", "urgent"],
            }),
            arguments: json!({
                "name": "name",
                "count": 0,
                "ratio": 0.0,
                "urgent": false,
            }),
        },
        SchemaRow {
            case: "a required subset of the declared properties",
            schema: json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"},
                    "units": {"type": "string"},
                },
                "required": ["location"],
            }),
            arguments: json!({"location": "location"}),
        },
        SchemaRow {
            case: "properties with no required array at all",
            schema: json!({
                "type": "object",
                "properties": {
                    "latitude": {"type": "number"},
                    "longitude": {"type": "number"},
                },
            }),
            arguments: json!({"latitude": 0.0, "longitude": 0.0}),
        },
        SchemaRow {
            case: "a schema declaring neither properties nor required names",
            schema: json!({"type": "object"}),
            arguments: json!({}),
        },
        SchemaRow {
            case: "an enumerated property takes a value the schema offers",
            schema: json!({
                "type": "object",
                "properties": {"units": {"type": "string", "enum": ["celsius", "fahrenheit"]}},
                "required": ["units"],
            }),
            arguments: json!({"units": "celsius"}),
        },
        SchemaRow {
            case: "a const and a default are the values the schema fixes",
            schema: json!({
                "type": "object",
                "properties": {
                    "mode": {"const": "fast"},
                    "retries": {"type": "integer", "default": 3},
                },
                "required": ["mode", "retries"],
            }),
            arguments: json!({"mode": "fast", "retries": 3}),
        },
        SchemaRow {
            case: "declared string bounds are met",
            schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "maxLength": 2},
                    "token": {"type": "string", "minLength": 8},
                },
                "required": ["code", "token"],
            }),
            arguments: json!({"code": "co", "token": "token___"}),
        },
        SchemaRow {
            case: "declared numeric bounds are met",
            schema: json!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer", "minimum": 5},
                    "level": {"type": "integer", "maximum": -3},
                    "weight": {"type": "number", "minimum": 1.5},
                },
                "required": ["count", "level", "weight"],
            }),
            arguments: json!({"count": 5, "level": -3, "weight": 1.5}),
        },
        SchemaRow {
            case: "array properties carry elements of the declared item type",
            schema: json!({
                "type": "object",
                "properties": {
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "ids": {"type": "array", "items": {"type": "integer"}, "minItems": 2},
                },
                "required": ["tags", "ids"],
            }),
            arguments: json!({"tags": ["tags"], "ids": [0, 0]}),
        },
        SchemaRow {
            case: "a nested object instantiates its own required names",
            schema: json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "object",
                        "properties": {
                            "since": {"type": "string"},
                            "limit": {"type": "integer"},
                        },
                        "required": ["since"],
                    },
                },
                "required": ["filter"],
            }),
            arguments: json!({"filter": {"since": "since"}}),
        },
        SchemaRow {
            case: "a local reference is followed into the schema's definitions",
            schema: json!({
                "type": "object",
                "properties": {"point": {"$ref": "#/$defs/Point"}},
                "required": ["point"],
                "$defs": {"Point": {
                    "type": "object",
                    "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}},
                    "required": ["x", "y"],
                }},
            }),
            arguments: json!({"point": {"x": 0, "y": 0}}),
        },
        SchemaRow {
            case: "a type given as a list of names",
            schema: json!({
                "type": "object",
                "properties": {"note": {"type": ["string", "null"]}},
                "required": ["note"],
            }),
            arguments: json!({"note": "note"}),
        },
        SchemaRow {
            case: "a composed property instantiates its first branch",
            schema: json!({
                "type": "object",
                "properties": {"choice": {"anyOf": [{"type": "integer"}, {"type": "string"}]}},
                "required": ["choice"],
            }),
            arguments: json!({"choice": 0}),
        },
        SchemaRow {
            case: "a required name declared through additionalProperties",
            schema: json!({
                "type": "object",
                "required": ["extra"],
                "additionalProperties": {"type": "boolean"},
            }),
            arguments: json!({"extra": false}),
        },
        SchemaRow {
            case: "a required name the schema declares no type for",
            schema: json!({
                "type": "object",
                "properties": {},
                "required": ["opaque"],
            }),
            arguments: json!({"opaque": Value::Null}),
        },
    ]
}

/// The user turn every row is sent with.
fn user_turn() -> Value {
    json!([{"role": "user", "content": [{"text": "What is the weather like there?"}]}])
}

/// A `toolConfig` offering ONE tool declared with `schema`, with the choice that
/// obliges the turn to request it.
fn forced_config(name: &str, schema: &Value) -> Value {
    json!({
        "tools": [{"toolSpec": {
            "name": name,
            "description": format!("Call {name}."),
            "inputSchema": {"json": schema},
        }}],
        "toolChoice": {"any": {}},
    })
}

/// Whether two JSON documents are the same value, comparing numbers by their
/// numeric value: an integral number is one value however it is spelled on the
/// wire (`0` and `0.0`).
fn same_json(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Number(left), Value::Number(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => (left - right).abs() <= f64::EPSILON,
            _ => left == right,
        },
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(name, value)| {
                    right.get(name).is_some_and(|other| same_json(value, other))
                })
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(value, other)| same_json(value, other))
        }
        _ => expected == actual,
    }
}

/// Assert the `input` document a tool request carries is the one the schema
/// describes, naming the case and the document actually served.
fn assert_arguments(case: &str, expected: &Value, actual: &Value) {
    assert!(
        same_json(expected, actual),
        "case {case:?}: expected the toolUse input to instantiate the declared schema as \
         {expected}, got {actual}"
    );
}

/// Every name a schema requires, as the rule reads them off the declaration.
fn required_names(schema: &Value) -> Vec<String> {
    schema["required"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The schema forms that are degenerate, contradictory or oversized: a tool's
/// `inputSchema.json` is an arbitrary caller document, so the projection has to
/// be total over one, and its own bounds are what keep the answer small.
///
/// The bounds are read from the constants the emulator publishes
/// ([`state::MAX_ARGUMENT_STRING_LENGTH`], [`state::MAX_ARGUMENT_ARRAY_ITEMS`]),
/// so a row states the rule rather than a number.
fn degenerate_rows() -> Vec<SchemaRow> {
    let padded = format!(
        "token{}",
        "_".repeat(state::MAX_ARGUMENT_STRING_LENGTH - "token".len())
    );
    vec![
        SchemaRow {
            case: "a schema that is not an object at all",
            schema: json!("not a schema"),
            arguments: json!({}),
        },
        SchemaRow {
            case: "the schema that admits anything",
            schema: json!(true),
            arguments: json!({}),
        },
        SchemaRow {
            case: "a top-level type that is not object",
            schema: json!({"type": "string"}),
            arguments: json!({}),
        },
        SchemaRow {
            case: "a type name no JSON Schema defines",
            schema: json!({
                "type": "object",
                "properties": {"x": {"type": "widget"}},
                "required": ["x"],
            }),
            arguments: json!({"x": Value::Null}),
        },
        SchemaRow {
            case: "a reference that points at itself",
            schema: json!({
                "type": "object",
                "properties": {"loop": {"$ref": "#/properties/loop"}},
                "required": ["loop"],
            }),
            arguments: json!({"loop": Value::Null}),
        },
        SchemaRow {
            case: "a reference that resolves nowhere",
            schema: json!({
                "type": "object",
                "properties": {"missing": {"$ref": "#/$defs/Absent"}},
                "required": ["missing"],
            }),
            arguments: json!({"missing": Value::Null}),
        },
        SchemaRow {
            case: "a minimum string length far above the bound",
            schema: json!({
                "type": "object",
                "properties": {"token": {"type": "string", "minLength": 10_000_000u64}},
                "required": ["token"],
            }),
            arguments: json!({"token": padded}),
        },
        SchemaRow {
            case: "a maximum string length of zero",
            schema: json!({
                "type": "object",
                "properties": {"code": {"type": "string", "maxLength": 0}},
                "required": ["code"],
            }),
            arguments: json!({"code": ""}),
        },
        SchemaRow {
            case: "a minimum element count far above the bound",
            schema: json!({
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1_000_000_000u64,
                    },
                },
                "required": ["tags"],
            }),
            arguments: json!({"tags": vec!["tags"; state::MAX_ARGUMENT_ARRAY_ITEMS]}),
        },
        SchemaRow {
            case: "numeric bounds that contradict each other",
            schema: json!({
                "type": "object",
                "properties": {"level": {"type": "integer", "minimum": 10, "maximum": 1}},
                "required": ["level"],
            }),
            arguments: json!({"level": 1}),
        },
        SchemaRow {
            case: "a fixed value the budget can pay for is carried whole",
            schema: json!({
                "type": "object",
                "properties": {"profile": {"const": {"tier": "gold", "scores": [1, 2, 3]}}},
                "required": ["profile"],
            }),
            arguments: json!({"profile": {"tier": "gold", "scores": [1, 2, 3]}}),
        },
        SchemaRow {
            case: "a fixed value larger than the whole document budget",
            schema: json!({
                "type": "object",
                "properties": {
                    "huge": {"const": vec![0; state::MAX_ARGUMENT_VALUES + 1]},
                },
                "required": ["huge"],
            }),
            arguments: json!({"huge": Value::Null}),
        },
        SchemaRow {
            case: "a fixed string longer than the document's byte ceiling",
            schema: json!({
                "type": "object",
                "properties": {
                    "blob": {"const": "x".repeat(state::MAX_ARGUMENT_BYTES + 1)},
                },
                "required": ["blob"],
            }),
            arguments: json!({"blob": Value::Null}),
        },
    ]
}

/// RULE: the `toolUse` block of a forced tool call carries the argument object
/// the requested tool's own `inputSchema` describes — every required name
/// present with a value of the declared type.
#[tokio::test]
async fn a_forced_tool_call_carries_the_arguments_its_schema_declares() {
    let router = emulator();

    for row in schema_rows() {
        let case = row.case;
        let call = post_json(
            &router,
            &format!("/model/{MODEL_ID}/converse"),
            &json!({
                "messages": user_turn(),
                "toolConfig": forced_config("get_weather", &row.schema),
            }),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        assert_eq!(
            body["stopReason"].as_str(),
            Some("tool_use"),
            "case {case:?}: a mandatory toolChoice must stop the turn on tool_use, got {}",
            body["stopReason"]
        );
        let tool_use = &body["output"]["message"]["content"][0]["toolUse"];
        let input = &tool_use["input"];
        assert!(
            input.is_object(),
            "case {case:?}: expected an argument object, got {tool_use}"
        );
        // Every name the schema requires is a key of the document, whatever else
        // the row expects.
        for name in required_names(&row.schema) {
            assert!(
                input.get(&name).is_some(),
                "case {case:?}: expected the required argument {name:?} to be a key of the \
                 input, got {input}"
            );
        }
        assert_arguments(case, &row.arguments, input);
    }
}

/// RULE (same rule, as `ConverseStream` frames it): the `toolUse` deltas of the
/// opened content block concatenate to exactly the document `Converse` answers
/// with for the identical request.
#[tokio::test]
async fn the_streamed_fragments_reassemble_the_same_argument_document() {
    let router = emulator();

    for row in schema_rows() {
        let case = row.case;
        let request = json!({
            "messages": user_turn(),
            "toolConfig": forced_config("file_ticket", &row.schema),
        });
        let stream = post_json(
            &router,
            &format!("/model/{MODEL_ID}/converse-stream"),
            &request,
        )
        .await;
        assert_eq!(
            stream.status,
            200,
            "case {case:?}: expected ConverseStream to answer 200, got {} / {}",
            stream.status,
            String::from_utf8_lossy(&stream.body)
        );
        let frames = decode_frames(case, &stream.body);
        let fragments: Vec<String> = frames
            .iter()
            .filter(|(name, _)| name == "contentBlockDelta")
            .map(|(_, payload)| {
                payload["delta"]["toolUse"]["input"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a toolUse delta carried no string input: {payload}")
                    })
                    .to_string()
            })
            .collect();
        assert!(
            !fragments.is_empty(),
            "case {case:?}: the stream carried no toolUse input fragment"
        );
        let reassembled = fragments.concat();
        let parsed: Value = serde_json::from_str(&reassembled).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the fragments must concatenate to a JSON object ({error}): \
                 {reassembled:?}"
            )
        });
        assert_arguments(case, &row.arguments, &parsed);

        // The unary path answers the identical request with the identical
        // document, so a streaming client and a unary one dispatch the same tool
        // call.
        let unary = post_json(&router, &format!("/model/{MODEL_ID}/converse"), &request).await;
        let served = unary.json()["output"]["message"]["content"][0]["toolUse"]["input"].clone();
        assert_arguments(case, &served, &parsed);
    }
}

/// RULE (which tool's schema): the arguments are the REQUESTED tool's, so a
/// request offering several tools answers with the schema of the one the choice
/// settles on — the named tool for a `tool` choice, the first offered for `any`.
#[tokio::test]
async fn the_arguments_come_from_the_schema_of_the_tool_actually_requested() {
    let router = emulator();
    let weather = json!({
        "type": "object",
        "properties": {"location": {"type": "string"}},
        "required": ["location"],
    });
    let ticket = json!({
        "type": "object",
        "properties": {
            "city": {"type": "string"},
            "description": {"type": "string"},
        },
        "required": ["city", "description"],
    });
    let tools = json!([
        {"toolSpec": {
            "name": "get_weather",
            "description": "Get the current weather for a named location.",
            "inputSchema": {"json": weather},
        }},
        {"toolSpec": {
            "name": "file_ticket",
            "description": "File a ticket for a city.",
            "inputSchema": {"json": ticket},
        }},
    ]);

    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "tool naming the second of two",
            json!({"tools": tools.clone(), "toolChoice": {"tool": {"name": "file_ticket"}}}),
            json!({"city": "city", "description": "description"}),
        ),
        (
            "tool naming the first of two",
            json!({"tools": tools.clone(), "toolChoice": {"tool": {"name": "get_weather"}}}),
            json!({"location": "location"}),
        ),
        (
            "any over two tools takes the first offered",
            json!({"tools": tools.clone(), "toolChoice": {"any": {}}}),
            json!({"location": "location"}),
        ),
        (
            // A system-defined tool declares no schema at all, so it names no
            // arguments.
            "any over a system-defined tool",
            json!({
                "tools": [{"systemTool": {"name": "web_search"}}],
                "toolChoice": {"any": {}},
            }),
            json!({}),
        ),
    ];

    for (case, tool_config, expected) in rows {
        let call = post_json(
            &router,
            &format!("/model/{MODEL_ID}/converse"),
            &json!({"messages": user_turn(), "toolConfig": tool_config}),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let tool_use = call.json()["output"]["message"]["content"][0]["toolUse"].clone();
        assert_arguments(case, &expected, &tool_use["input"]);
    }
}

/// RULE (the projection is total over a caller's document): a tool's
/// `inputSchema.json` is an arbitrary document, so a schema that is degenerate,
/// self-referential, contradictory or oversized is still served — as an argument
/// object whose values stay inside the bounds this emulator publishes.
#[tokio::test]
async fn a_degenerate_or_oversized_schema_is_served_within_the_bounds() {
    let router = emulator();

    for row in degenerate_rows() {
        let case = row.case;
        let call = post_json(
            &router,
            &format!("/model/{MODEL_ID}/converse"),
            &json!({
                "messages": user_turn(),
                "toolConfig": forced_config("get_weather", &row.schema),
            }),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let input = call.json()["output"]["message"]["content"][0]["toolUse"]["input"].clone();
        assert!(
            input.is_object(),
            "case {case:?}: expected an argument object, got {input}"
        );
        assert_arguments(case, &row.arguments, &input);
        assert_within_bounds(case, &input);
    }
}

/// RULE (the bounds this emulator puts on its own response): the argument
/// document one request composes carries at most
/// [`state::MAX_ARGUMENT_VALUES`] values and at most
/// [`state::MAX_ARGUMENT_BYTES`] bytes, so a schema whose nesting, member names
/// or fixed values would multiply without limit is served as a bounded document
/// rather than growing the response without bound.
#[tokio::test]
async fn a_nesting_schema_is_instantiated_within_the_emulators_bound() {
    let router = emulator();
    // Arrays of arrays, each level asking for several elements: the number of
    // leaves such a schema describes is exponential in its depth.
    let mut nested = json!({"type": "array", "items": {"type": "string"}, "minItems": 8});
    for _ in 0..40 {
        nested = json!({"type": "array", "items": nested, "minItems": 8});
    }
    let schema = json!({
        "type": "object",
        "properties": {"matrix": nested},
        "required": ["matrix"],
    });

    let call = post_json(
        &router,
        &format!("/model/{MODEL_ID}/converse"),
        &json!({
            "messages": user_turn(),
            "toolConfig": forced_config("walk_matrix", &schema),
        }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "a deeply nesting schema must still be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let input = call.json()["output"]["message"]["content"][0]["toolUse"]["input"].clone();
    assert!(
        input.get("matrix").is_some(),
        "the required argument must still be a key of the input, got {input}"
    );
    assert_within_bounds("a deeply nesting schema", &input);

    // The same ceiling covers the MEMBERS a schema names, not only the elements
    // it asks for: a property-less object with a long `required` list, nested
    // under a tree of arrays, describes far more members than the budget.
    let wide: Vec<String> = (0..200).map(|index| format!("field{index}")).collect();
    let mut nested = json!({"type": "object", "required": wide});
    for _ in 0..6 {
        nested = json!({"type": "array", "items": nested, "minItems": 8});
    }
    let call = post_json(
        &router,
        &format!("/model/{MODEL_ID}/converse"),
        &json!({
            "messages": user_turn(),
            "toolConfig": forced_config(
                "walk_wide",
                &json!({
                    "type": "object",
                    "properties": {"matrix": nested},
                    "required": ["matrix"],
                }),
            ),
        }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "a schema naming more members than the budget must still be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let input = call.json()["output"]["message"]["content"][0]["toolUse"]["input"].clone();
    assert_within_bounds("a schema naming more members than the budget", &input);

    // And it covers a value the schema FIXES: one large value in the schema's
    // definitions, referenced from many required properties, would otherwise be
    // reproduced once per reference.
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for index in 0..200 {
        let name = format!("copy{index}");
        properties.insert(name.clone(), json!({"$ref": "#/$defs/Big"}));
        required.push(name);
    }
    let call = post_json(
        &router,
        &format!("/model/{MODEL_ID}/converse"),
        &json!({
            "messages": user_turn(),
            "toolConfig": forced_config(
                "copy_big",
                &json!({
                    "type": "object",
                    "properties": properties,
                    "required": required,
                    "$defs": {"Big": {"const": vec![0; 64]}},
                }),
            ),
        }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "a schema reproducing a fixed value must still be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let input = call.json()["output"]["message"]["content"][0]["toolUse"]["input"].clone();
    assert_within_bounds("a schema reproducing a fixed value", &input);
}

/// Assert the argument document stays inside both ceilings this emulator
/// publishes for one of them.
fn assert_within_bounds(case: &str, input: &Value) {
    let values = count_values(input);
    assert!(
        values <= state::MAX_ARGUMENT_VALUES,
        "case {case:?}: expected at most {} values in the argument document, got {values}",
        state::MAX_ARGUMENT_VALUES
    );
    let bytes = input.to_string().len();
    assert!(
        bytes <= state::MAX_ARGUMENT_BYTES,
        "case {case:?}: expected at most {} bytes of argument document, got {bytes}",
        state::MAX_ARGUMENT_BYTES
    );
}

/// Every value a document carries, at every level: one per member and element.
fn count_values(value: &Value) -> usize {
    match value {
        Value::Object(members) => 1 + members.values().map(count_values).sum::<usize>(),
        Value::Array(items) => 1 + items.iter().map(count_values).sum::<usize>(),
        _ => 1,
    }
}

/// RULE (the round trip the arguments exist for): the tool request a forced turn
/// answers with can be dispatched and reported back — the conversation carrying
/// the assistant's `toolUse` block and a `toolResult` computed from its
/// arguments is accepted, and the turn continues.
#[tokio::test]
async fn the_argument_document_drives_the_tool_result_turn() {
    let router = emulator();
    let schema = json!({
        "type": "object",
        "properties": {"location": {"type": "string"}, "units": {"type": "string"}},
        "required": ["location", "units"],
    });
    let tool_config = forced_config("get_weather", &schema);
    let forced = post_json(
        &router,
        &format!("/model/{MODEL_ID}/converse"),
        &json!({"messages": user_turn(), "toolConfig": tool_config}),
    )
    .await;
    assert_eq!(
        forced.status,
        200,
        "the forced turn must be served, got {} / {}",
        forced.status,
        String::from_utf8_lossy(&forced.body)
    );
    let assistant = forced.json()["output"]["message"].clone();
    let tool_use = assistant["content"][0]["toolUse"].clone();
    let arguments = tool_use["input"].clone();
    // A client dispatches the tool with the arguments it was handed: both
    // required keys are there to read.
    let location = arguments["location"]
        .as_str()
        .unwrap_or_else(|| panic!("the arguments carry no location: {arguments}"));
    let units = arguments["units"]
        .as_str()
        .unwrap_or_else(|| panic!("the arguments carry no units: {arguments}"));
    let continued = post_json(
        &router,
        &format!("/model/{MODEL_ID}/converse"),
        &json!({
            "messages": [
                user_turn()[0],
                assistant,
                {"role": "user", "content": [{"toolResult": {
                    "toolUseId": tool_use["toolUseId"],
                    "content": [{"json": {"location": location, "units": units, "value": 23}}],
                    "status": "success",
                }}]},
            ],
            "toolConfig": {"tools": [{"toolSpec": {
                "name": "get_weather",
                "description": "Call get_weather.",
                "inputSchema": {"json": schema},
            }}]},
        }),
    )
    .await;
    assert_eq!(
        continued.status,
        200,
        "the turn reporting the tool's result must be served, got {} / {}",
        continued.status,
        String::from_utf8_lossy(&continued.body)
    );
    let body = continued.json();
    assert_eq!(
        body["stopReason"].as_str(),
        Some("end_turn"),
        "the continued turn generates text, got {}",
        body["stopReason"]
    );
}
