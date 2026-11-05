// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! [`Codec`] for `rpcv2Cbor`, layered over `aws-smithy-cbor`.
//!
//! `aws-smithy-cbor` 0.62.2 is the CBOR implementation pinned by the version
//! set. Two of its behaviors do not match what real clients put on the wire,
//! and this module corrects both without forking the crate:
//!
//! * **Collection elements carry no member name.** The upstream serializer
//!   writes the schema's member name as a text key before every value whose
//!   schema is a struct member, wherever that value sits. The generated
//!   serializers pass a list member's own schema for each scalar element and a
//!   map member's schema for each key and value, so a `list<double>` comes out
//!   as `["Values", 1.0, "Values", 2.0]` instead of `[1.0, 2.0]`. Clients
//!   cannot decode that. [`CborShapeSerializer`] hands the upstream serializer a
//!   schema without a member name for everything written inside a list or map,
//!   so only struct members are keyed.
//! * **Whole numbers decode as floats.** The upstream deserializer reads a
//!   `float` or `double` only from a CBOR floating-point item. Encoders emit
//!   the shortest form, so a value such as `5.0` can arrive as the CBOR integer
//!   `5`; the Java SDK v2 does this. [`CborShapeDeserializer`] reads a CBOR
//!   integer in a `float` or `double` position as that number.
//!
//! Both wrappers delegate everything else to the upstream codec.

use aws_smithy_cbor::codec::CborCodec as UpstreamCborCodec;
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::Schema;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};

/// `rpcv2Cbor` codec. Produces [`CborShapeSerializer`] and
/// [`CborShapeDeserializer`].
#[derive(Debug, Default)]
pub struct CborCodec {
    upstream: UpstreamCborCodec,
}

impl Codec for CborCodec {
    type Serializer = CborShapeSerializer;
    type Deserializer<'a> = CborShapeDeserializer<'a>;

    fn create_serializer(&self) -> Self::Serializer {
        CborShapeSerializer {
            inner: self.upstream.create_serializer(),
        }
    }

    fn create_deserializer<'a>(&self, input: &'a [u8]) -> Self::Deserializer<'a> {
        CborShapeDeserializer {
            inner: self.upstream.create_deserializer(input),
        }
    }
}

// =================================================================================================
// Serializer
// =================================================================================================

/// The schema to give the upstream serializer for a value written at the
/// current position: the schema itself inside a struct, and the same shape
/// without its member name inside a list or map.
fn schema_for_position(schema: &Schema, in_collection: bool) -> Option<Schema> {
    in_collection.then(|| Schema::new(*schema.shape_id(), schema.shape_type()))
}

/// One level of the serializer walk. Wraps the upstream serializer for the
/// duration of a single call and records whether the value being written is an
/// element of a list or map.
struct SerializerScope<'a> {
    inner: &'a mut dyn ShapeSerializer,
    in_collection: bool,
}

/// Forwards the members of a struct to the caller's [`SerializableStruct`],
/// handing it a struct-level scope so nested collections are tracked.
struct ScopedMembers<'v>(&'v dyn SerializableStruct);

impl SerializableStruct for ScopedMembers<'_> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut SerializerScope {
            inner: serializer,
            in_collection: false,
        })
    }
}

macro_rules! forward_scalar_writes {
    ($($method:ident($value_ty:ty)),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema, value: $value_ty) -> Result<(), SerdeError> {
                let stripped = schema_for_position(schema, self.in_collection);
                self.inner.$method(stripped.as_ref().unwrap_or(schema), value)
            }
        )+
    };
}

impl ShapeSerializer for SerializerScope<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        let stripped = schema_for_position(schema, self.in_collection);
        self.inner
            .write_struct(stripped.as_ref().unwrap_or(schema), &ScopedMembers(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let stripped = schema_for_position(schema, self.in_collection);
        self.inner
            .write_list(stripped.as_ref().unwrap_or(schema), &|elements| {
                write_elements(&mut SerializerScope {
                    inner: elements,
                    in_collection: true,
                })
            })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let stripped = schema_for_position(schema, self.in_collection);
        self.inner
            .write_map(stripped.as_ref().unwrap_or(schema), &|entries| {
                write_entries(&mut SerializerScope {
                    inner: entries,
                    in_collection: true,
                })
            })
    }

    forward_scalar_writes!(
        write_boolean(bool),
        write_byte(i8),
        write_short(i16),
        write_integer(i32),
        write_long(i64),
        write_float(f32),
        write_double(f64),
        write_big_integer(&BigInteger),
        write_big_decimal(&BigDecimal),
        write_string(&str),
        write_blob(&[u8]),
        write_timestamp(&DateTime),
        write_document(&Document),
    );

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        let stripped = schema_for_position(schema, self.in_collection);
        self.inner.write_null(stripped.as_ref().unwrap_or(schema))
    }
}

/// `rpcv2Cbor` [`ShapeSerializer`]: the upstream CBOR serializer with member
/// names confined to struct members.
pub struct CborShapeSerializer {
    inner: <UpstreamCborCodec as Codec>::Serializer,
}

impl CborShapeSerializer {
    fn scope(&mut self) -> SerializerScope<'_> {
        SerializerScope {
            inner: &mut self.inner,
            in_collection: false,
        }
    }
}

impl FinishSerializer for CborShapeSerializer {
    fn finish(self) -> Vec<u8> {
        self.inner.finish()
    }
}

macro_rules! forward_root_writes {
    ($($method:ident($value_ty:ty)),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema, value: $value_ty) -> Result<(), SerdeError> {
                self.scope().$method(schema, value)
            }
        )+
    };
}

impl ShapeSerializer for CborShapeSerializer {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.scope().write_struct(schema, value)
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.scope().write_list(schema, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.scope().write_map(schema, write_entries)
    }

    forward_root_writes!(
        write_boolean(bool),
        write_byte(i8),
        write_short(i16),
        write_integer(i32),
        write_long(i64),
        write_float(f32),
        write_double(f64),
        write_big_integer(&BigInteger),
        write_big_decimal(&BigDecimal),
        write_string(&str),
        write_blob(&[u8]),
        write_timestamp(&DateTime),
        write_document(&Document),
    );

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.scope().write_null(schema)
    }
}

// =================================================================================================
// Deserializer
// =================================================================================================

/// One level of the deserializer walk. Re-wraps the upstream deserializer that
/// is handed to each struct member, list element and map value, so the numeric
/// rule applies at every depth.
struct DeserializerScope<'a> {
    inner: &'a mut dyn ShapeDeserializer,
}

macro_rules! forward_reads {
    ($($method:ident -> $ret:ty),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema) -> Result<$ret, SerdeError> {
                self.inner.$method(schema)
            }
        )+
    };
}

impl ShapeDeserializer for DeserializerScope<'_> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .read_struct(schema, &mut |member, member_reader| {
                state(
                    member,
                    &mut DeserializerScope {
                        inner: member_reader,
                    },
                )
            })
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.read_list(schema, &mut |element_reader| {
            state(&mut DeserializerScope {
                inner: element_reader,
            })
        })
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.read_map(schema, &mut |key, value_reader| {
            state(
                key,
                &mut DeserializerScope {
                    inner: value_reader,
                },
            )
        })
    }

    /// A CBOR floating-point item, or failing that a CBOR integer.
    ///
    /// The upstream reader does not move past an item it rejects, so the
    /// integer read starts at the same item.
    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        match self.inner.read_float(schema) {
            Ok(value) => Ok(value),
            Err(float_error) => self
                .inner
                .read_long(schema)
                .map(|whole| whole as f32)
                .map_err(|_| float_error),
        }
    }

    /// A CBOR floating-point item, or failing that a CBOR integer.
    fn read_double(&mut self, schema: &Schema) -> Result<f64, SerdeError> {
        match self.inner.read_double(schema) {
            Ok(value) => Ok(value),
            Err(double_error) => self
                .inner
                .read_long(schema)
                .map(|whole| whole as f64)
                .map_err(|_| double_error),
        }
    }

    forward_reads!(
        read_boolean -> bool,
        read_byte -> i8,
        read_short -> i16,
        read_integer -> i32,
        read_long -> i64,
        read_big_integer -> BigInteger,
        read_big_decimal -> BigDecimal,
        read_string -> String,
        read_blob -> Blob,
        read_timestamp -> DateTime,
        read_document -> Document,
    );

    fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        self.inner.read_null()
    }

    fn container_size(&self) -> Option<usize> {
        self.inner.container_size()
    }
}

/// `rpcv2Cbor` [`ShapeDeserializer`]: the upstream CBOR deserializer that also
/// reads a CBOR integer where a `float` or `double` is expected.
pub struct CborShapeDeserializer<'a> {
    inner: <UpstreamCborCodec as Codec>::Deserializer<'a>,
}

impl CborShapeDeserializer<'_> {
    fn scope(&mut self) -> DeserializerScope<'_> {
        DeserializerScope {
            inner: &mut self.inner,
        }
    }
}

macro_rules! forward_root_reads {
    ($($method:ident -> $ret:ty),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema) -> Result<$ret, SerdeError> {
                self.scope().$method(schema)
            }
        )+
    };
}

impl ShapeDeserializer for CborShapeDeserializer<'_> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.scope().read_struct(schema, state)
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.scope().read_list(schema, state)
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.scope().read_map(schema, state)
    }

    forward_root_reads!(
        read_boolean -> bool,
        read_byte -> i8,
        read_short -> i16,
        read_integer -> i32,
        read_long -> i64,
        read_float -> f32,
        read_double -> f64,
        read_big_integer -> BigInteger,
        read_big_decimal -> BigDecimal,
        read_string -> String,
        read_blob -> Blob,
        read_timestamp -> DateTime,
        read_document -> Document,
    );

    fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        self.scope().read_null()
    }

    fn container_size(&self) -> Option<usize> {
        self.inner.container_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_cbor::Encoder;
    use aws_smithy_schema::{ShapeId, ShapeType};

    const fn member(name: &'static str, shape_type: ShapeType, index: usize) -> Schema {
        Schema::new_member(
            ShapeId::from_static("com.test#Output$member", "com.test", "Output"),
            shape_type,
            name,
            index,
        )
    }

    static VALUES: Schema = member("Values", ShapeType::List, 0);
    static TIMESTAMPS: Schema = member("Timestamps", ShapeType::List, 1);
    static LABELS: Schema = member("Labels", ShapeType::Map, 2);
    static ROWS: Schema = member("Rows", ShapeType::List, 3);
    static THRESHOLD: Schema = member("Threshold", ShapeType::Double, 0);
    static RATIO: Schema = member("Ratio", ShapeType::Float, 1);
    static AFTER: Schema = member("After", ShapeType::Double, 2);
    static NAME: Schema = member("Name", ShapeType::String, 0);
    static ROW_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("com.test#Row", "com.test", "Row"),
        ShapeType::Structure,
        &[],
    );
    static THRESHOLDS_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("com.test#Thresholds", "com.test", "Thresholds"),
        ShapeType::Structure,
        &[&THRESHOLD, &RATIO, &AFTER],
    );
    static OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("com.test#Output", "com.test", "Output"),
        ShapeType::Structure,
        &[],
    );

    struct Row {
        name: &'static str,
        values: Vec<f64>,
    }

    impl SerializableStruct for Row {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&NAME, self.name)?;
            s.write_list(&VALUES, &|elements| {
                for value in &self.values {
                    elements.write_double(&VALUES, *value)?;
                }
                Ok(())
            })
        }
    }

    struct Output {
        values: Vec<f64>,
        timestamps: Vec<DateTime>,
        labels: Vec<(&'static str, &'static str)>,
        rows: Vec<Row>,
    }

    impl SerializableStruct for Output {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_list(&VALUES, &|elements| {
                for value in &self.values {
                    elements.write_double(&VALUES, *value)?;
                }
                Ok(())
            })?;
            s.write_list(&TIMESTAMPS, &|elements| {
                for timestamp in &self.timestamps {
                    elements.write_timestamp(&TIMESTAMPS, timestamp)?;
                }
                Ok(())
            })?;
            s.write_map(&LABELS, &|entries| {
                for (key, value) in &self.labels {
                    entries.write_string(&LABELS, key)?;
                    entries.write_string(&LABELS, value)?;
                }
                Ok(())
            })?;
            s.write_list(&ROWS, &|elements| {
                for row in &self.rows {
                    elements.write_struct(&ROW_SCHEMA, row)?;
                }
                Ok(())
            })
        }
    }

    fn serialize(output: &Output) -> Vec<u8> {
        let mut serializer = CborCodec::default().create_serializer();
        serializer
            .write_struct(&OUTPUT_SCHEMA, output)
            .expect("serialize");
        serializer.finish()
    }

    /// A list of scalars is a bare CBOR array: no member name precedes any element.
    #[test]
    fn scalar_list_elements_carry_no_member_name() {
        let output = Output {
            values: vec![1.5, 2.5],
            timestamps: vec![DateTime::from_secs(10)],
            labels: vec![("k", "v")],
            rows: vec![],
        };

        let mut expected = Encoder::new(Vec::new());
        expected.begin_map();
        expected.str("Values").begin_array();
        expected.double(1.5);
        expected.double(2.5);
        expected.end();
        expected.str("Timestamps").begin_array();
        expected.timestamp(&DateTime::from_secs(10));
        expected.end();
        expected.str("Labels").begin_map();
        expected.str("k");
        expected.str("v");
        expected.end();
        expected.str("Rows").begin_array();
        expected.end();
        expected.end();

        assert_eq!(serialize(&output), expected.into_writer());
    }

    /// Lists nested in structs that are themselves list elements stay bare.
    #[test]
    fn collections_inside_list_elements_are_serialized_without_stray_keys() {
        let output = Output {
            values: vec![],
            timestamps: vec![],
            labels: vec![],
            rows: vec![Row {
                name: "a",
                values: vec![3.5],
            }],
        };

        let mut expected = Encoder::new(Vec::new());
        expected.begin_map();
        expected.str("Values").begin_array();
        expected.end();
        expected.str("Timestamps").begin_array();
        expected.end();
        expected.str("Labels").begin_map();
        expected.end();
        expected.str("Rows").begin_array();
        expected.begin_map();
        expected.str("Name");
        expected.str("a");
        expected.str("Values").begin_array();
        expected.double(3.5);
        expected.end();
        expected.end();
        expected.end();
        expected.end();

        assert_eq!(serialize(&output), expected.into_writer());
    }

    struct Thresholds {
        threshold: f64,
        ratio: f32,
        after: f64,
    }

    fn read_thresholds(bytes: &[u8]) -> Result<Thresholds, SerdeError> {
        let mut deserializer = CborCodec::default().create_deserializer(bytes);
        let mut threshold = None;
        let mut ratio = None;
        let mut after = None;
        deserializer.read_struct(&THRESHOLDS_SCHEMA, &mut |member, reader| {
            match member.member_name() {
                Some("Threshold") => threshold = Some(reader.read_double(&THRESHOLD)?),
                Some("Ratio") => ratio = Some(reader.read_float(&RATIO)?),
                Some("After") => after = Some(reader.read_double(&AFTER)?),
                other => panic!("unexpected member {other:?}"),
            }
            Ok(())
        })?;
        Ok(Thresholds {
            threshold: threshold.expect("Threshold"),
            ratio: ratio.expect("Ratio"),
            after: after.expect("After"),
        })
    }

    fn thresholds_body(
        threshold: impl FnOnce(&mut Encoder),
        ratio: impl FnOnce(&mut Encoder),
    ) -> Vec<u8> {
        let mut encoder = Encoder::new(Vec::new());
        encoder.begin_map();
        encoder.str("Threshold");
        threshold(&mut encoder);
        encoder.str("Ratio");
        ratio(&mut encoder);
        encoder.str("After");
        encoder.double(2.5);
        encoder.end();
        encoder.into_writer()
    }

    /// Whole numbers sent as CBOR integers read as the same `double` / `float`,
    /// and the member after them still decodes.
    #[test]
    fn integer_items_read_as_double_and_float() {
        let body = thresholds_body(
            |e| {
                e.long(5);
            },
            |e| {
                e.long(-3);
            },
        );
        let thresholds = read_thresholds(&body).expect("integers must decode as floats");
        assert_eq!(thresholds.threshold, 5.0);
        assert_eq!(thresholds.ratio, -3.0);
        assert_eq!(thresholds.after, 2.5);
    }

    /// Floating-point items keep decoding as before.
    #[test]
    fn floating_point_items_still_read_as_double_and_float() {
        let body = thresholds_body(
            |e| {
                e.double(5.25);
            },
            |e| {
                e.float(0.5);
            },
        );
        let thresholds = read_thresholds(&body).expect("floats must decode");
        assert_eq!(thresholds.threshold, 5.25);
        assert_eq!(thresholds.ratio, 0.5);
        assert_eq!(thresholds.after, 2.5);
    }

    /// An item that is neither a float nor an integer is still refused, with
    /// the error for the type that was expected.
    #[test]
    fn non_numeric_item_is_still_rejected() {
        let body = thresholds_body(
            |e| {
                e.str("5");
            },
            |e| {
                e.float(0.5);
            },
        );
        let error = read_thresholds(&body)
            .err()
            .expect("a string is not a double");
        assert!(
            format!("{error}").contains("f64")
                || format!("{error}").to_lowercase().contains("double"),
            "error must name the expected float type: {error}"
        );
    }
}
