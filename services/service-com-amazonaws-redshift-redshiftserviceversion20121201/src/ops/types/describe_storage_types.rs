// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_storage` operation.
//!
//! HAND-AUTHORED, not scaffolded: `DescribeStorage` binds Smithy's `Unit` input,
//! which the op-stub generator declines to type (see `src/ops/SKIPPED_STUBS.md`),
//! so its shapes are declared here beside the generated ones. Each
//! structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json");

/// `DescribeStorage` has no input members: the model binds it to
/// `smithy.api#Unit`, a PRELUDE shape that is not in this service's model file.
/// The legacy `structure = "..."` key declares the shape id without resolving it
/// against the model, and the macro's empty-struct path emits the schema, the
/// `DESCRIBE_STORAGE_INPUT_SCHEMA` static and the serialize/deserialize impls
/// `Operation::Input` requires from the id alone.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeStorageInput {}

/// The decimal text AWS renders a Smithy `double` as.
///
/// A double always carries a FRACTIONAL PART on the wire — the recorded
/// `DescribeStorage` of an empty account answers `0.0`, not `0` — so a whole
/// value renders with one fractional digit and a value that already has
/// fractional digits keeps them. Rust's own `f64` rendering drops the fractional
/// part of a whole value (`0.0f64.to_string() == "0"`), which is why the
/// serializer below formats the value here instead of handing it to
/// `write_double`: that formatting lives in the framework's XML writer, which
/// this emulator cannot edit.
///
/// A non-finite double has no decimal form and cannot be a sum of storage sizes,
/// so it falls back to Rust's own rendering rather than being given a fake one.
fn double_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// `DescribeStorage`'s output: the `CustomerStorageMessage` shape.
///
/// Hand-authored like the input, because the generator emitted neither \u2014 and
/// written out rather than macro-generated for one reason: its two members are
/// the only doubles this service puts on the wire, and they must carry the
/// decimal form [`double_text`] renders (the macro's serializer would hand them
/// to `write_double`, which drops the fractional part of a whole value).
#[derive(Debug, Clone, Default)]
pub struct DescribeStorageOutput {
    pub total_backup_size_in_mega_bytes: Option<f64>,
    pub total_provisioned_storage_in_mega_bytes: Option<f64>,
}

#[allow(non_upper_case_globals)]
impl DescribeStorageOutput {
    const NAMESPACE: &'static str = "com.amazonaws.redshift";

    pub const M_TOTAL_BACKUP_SIZE_IN_MEGA_BYTES: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.redshift#CustomerStorageMessage$TotalBackupSizeInMegaBytes",
                Self::NAMESPACE,
                "TotalBackupSizeInMegaBytes",
            ),
            ::aws_smithy_schema::ShapeType::Double,
            "TotalBackupSizeInMegaBytes",
            0,
        );

    pub const M_TOTAL_PROVISIONED_STORAGE_IN_MEGA_BYTES: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.redshift#CustomerStorageMessage$TotalProvisionedStorageInMegaBytes",
                Self::NAMESPACE,
                "TotalProvisionedStorageInMegaBytes",
            ),
            ::aws_smithy_schema::ShapeType::Double,
            "TotalProvisionedStorageInMegaBytes",
            1,
        );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.redshift#CustomerStorageMessage",
            Self::NAMESPACE,
            "CustomerStorageMessage",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_TOTAL_BACKUP_SIZE_IN_MEGA_BYTES,
            &Self::M_TOTAL_PROVISIONED_STORAGE_IN_MEGA_BYTES,
        ],
    );
}

pub static DESCRIBE_STORAGE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &DescribeStorageOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DescribeStorageOutput {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // Each member is written as the decimal TEXT of its double (see
        // [`double_text`]): the element's name, order and schema are the model's,
        // and only the number's rendering is this module's.
        if let Some(value) = self.total_backup_size_in_mega_bytes {
            s.write_string(
                &Self::M_TOTAL_BACKUP_SIZE_IN_MEGA_BYTES,
                &double_text(value),
            )?;
        }
        if let Some(value) = self.total_provisioned_storage_in_mega_bytes {
            s.write_string(
                &Self::M_TOTAL_PROVISIONED_STORAGE_IN_MEGA_BYTES,
                &double_text(value),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::double_text;

    /// A double carries a fractional part whichever value it holds: a whole
    /// value grows one, and a fractional one keeps the digits it has.
    #[test]
    fn a_double_always_renders_with_a_fractional_part() {
        // (the value, the text AWS renders it as)
        let rows: [(f64, &str); 7] = [
            (0.0, "0.0"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (160_000.0, "160000.0"),
            (0.5, "0.5"),
            (1.75, "1.75"),
            (2_048.25, "2048.25"),
        ];
        for (value, expected) in rows {
            let rendered = double_text(value);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
