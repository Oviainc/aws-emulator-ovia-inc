// SPDX-License-Identifier: CC0-1.0

//! The `Intent` shape, written out rather than expanded from the model for the
//! one thing its `slots` map needs that the generated codec cannot express.
//!
//! `Intent$slots` is "a map of all of the slots for the intent. The name of the
//! slot maps to the value of the slot. If a slot has not been filled, the value
//! is null" (`smithy_query(query="doc", shape="Intent", member="slots")`). The
//! generated serializer writes every map value as the structure it is, so a slot
//! the intent declares and the conversation has no value for would travel as an
//! empty object `{}` — and a client could not tell "declared and unfilled" from
//! "filled with nothing", which is exactly the distinction the documented `null`
//! draws. Everything else here mirrors what
//! `#[smithy_macros::smithy(shape = "…#Intent")]` emits: one member schema per
//! modelled member (index = position in the model's member list) with that
//! member's constraint traits, the shape's own `SCHEMA`, and the
//! `SerializableStruct` / `DeserializableStruct` pair the codecs drive.
//!
//! An unfilled slot is held as a [`Slot`] carrying no value of any kind
//! ([`is_unfilled`]), which is what the deserializer reads a `null` map value
//! back as, so the shape round-trips through its own wire form.
//!
//! Because the member list and the member indices below are written out, they
//! are the one thing here a model refresh does not carry: if
//! `com.amazonaws.lexruntimev2#Intent` ever gains or reorders a member in
//! `smithy-model/com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0.json`,
//! this module is where it has to be added, or that member never reaches the
//! wire.

use super::*;

/// This service's Smithy namespace, which every shape id below sits in.
const NAMESPACE: &str = "com.amazonaws.lexruntimev2";

/// The member schema sub-index a map's own key and value writes carry. The JSON
/// codec writes a map entry through the map member's own schema, so the members
/// below need no sub-index of their own.
const MEMBER_INDEX_NAME: usize = 0;
/// See [`MEMBER_INDEX_NAME`]: `slots` is the model's second member of `Intent`.
const MEMBER_INDEX_SLOTS: usize = 1;
/// See [`MEMBER_INDEX_NAME`].
const MEMBER_INDEX_STATE: usize = 2;
/// See [`MEMBER_INDEX_NAME`].
const MEMBER_INDEX_CONFIRMATION_STATE: usize = 3;

/// `Intent$name` is a `NonEmptyString` (len 1..), the one constraint the shape's
/// members carry beside the two enums.
static M_NAME_TRAITS: std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::LengthTrait::new(Some(1), None),
        ));
        traits
    });

/// `Intent$state` targets the `IntentState` enum, whose members are
/// [`INTENT_STATE_VALUES`].
static M_STATE_TRAITS: std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::EnumTrait::new(INTENT_STATE_VALUES),
        ));
        traits
    });

/// `Intent$confirmationState` targets the `ConfirmationState` enum, whose
/// members are [`CONFIRMATION_STATE_VALUES`].
static M_CONFIRMATION_STATE_TRAITS: std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::EnumTrait::new(CONFIRMATION_STATE_VALUES),
        ));
        traits
    });

/// "The current intent that Amazon Lex V2 is attempting to fulfill"
/// (`com.amazonaws.lexruntimev2#Intent`).
#[derive(Debug, Clone, Default)]
pub struct Intent {
    pub name: String,
    pub slots: Option<std::collections::HashMap<String, Slot>>,
    pub state: Option<String>,
    pub confirmation_state: Option<String>,
}

impl Intent {
    /// `Intent$name`.
    pub const M_NAME: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lexruntimev2#Intent$name",
            NAMESPACE,
            "name",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "name",
        MEMBER_INDEX_NAME,
    )
    .with_traits(&M_NAME_TRAITS);

    /// `Intent$slots`, the `Slots` map.
    pub const M_SLOTS: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lexruntimev2#Intent$slots",
            NAMESPACE,
            "slots",
        ),
        ::aws_smithy_schema::ShapeType::Map,
        "slots",
        MEMBER_INDEX_SLOTS,
    );

    /// `Intent$state`.
    pub const M_STATE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lexruntimev2#Intent$state",
            NAMESPACE,
            "state",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "state",
        MEMBER_INDEX_STATE,
    )
    .with_traits(&M_STATE_TRAITS);

    /// `Intent$confirmationState`.
    pub const M_CONFIRMATION_STATE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.lexruntimev2#Intent$confirmationState",
                NAMESPACE,
                "confirmationState",
            ),
            ::aws_smithy_schema::ShapeType::String,
            "confirmationState",
            MEMBER_INDEX_CONFIRMATION_STATE,
        )
        .with_traits(&M_CONFIRMATION_STATE_TRAITS);

    /// The shape's own schema, members in the order the model declares them.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lexruntimev2#Intent",
            NAMESPACE,
            "Intent",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_NAME,
            &Self::M_SLOTS,
            &Self::M_STATE,
            &Self::M_CONFIRMATION_STATE,
        ],
    );
}

/// Has this slot no value of any kind — neither the single `value` a `Scalar`
/// slot holds, nor the `values` of a `List` one, nor the `subSlots` of a
/// composite?
///
/// Such an entry is a slot the intent declares that nothing has filled, which is
/// the documented `null` of `Intent$slots`.
pub fn is_unfilled(slot: &Slot) -> bool {
    slot.value.is_none()
        && slot.values.is_none()
        && slot.sub_slots.is_none()
        && slot.shape.is_none()
}

impl ::aws_smithy_schema::serde::SerializableStruct for Intent {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(&Self::M_NAME, self.name.as_str())?;
        if let Some(slots) = self.slots.as_ref() {
            serializer.write_map(&Self::M_SLOTS, &|entry| {
                for (name, slot) in slots.iter() {
                    entry.write_string(&Self::M_SLOTS, name.as_str())?;
                    // The documented `null` for a slot with no value, and the
                    // slot itself for one that has one.
                    if is_unfilled(slot) {
                        entry.write_null(&Self::M_SLOTS)?;
                    } else {
                        entry.write_struct(&Self::M_SLOTS, slot)?;
                    }
                }
                Ok(())
            })?;
        }
        if let Some(state) = self.state.as_ref() {
            serializer.write_string(&Self::M_STATE, state.as_str())?;
        }
        if let Some(confirmation_state) = self.confirmation_state.as_ref() {
            serializer.write_string(&Self::M_CONFIRMATION_STATE, confirmation_state.as_str())?;
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for Intent {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut name: Option<String> = None;
        let mut slots: Option<std::collections::HashMap<String, Slot>> = None;
        let mut state: Option<String> = None;
        let mut confirmation_state: Option<String> = None;

        deserializer.read_struct(&Self::SCHEMA, &mut |member, reader| {
            match member.member_index() {
                Some(MEMBER_INDEX_NAME) => {
                    name = Some(
                        reader
                            .read_string(member)
                            .map_err(|e| ::aws_smithy_schema_server::within_member(e, "name"))?,
                    );
                }
                Some(MEMBER_INDEX_SLOTS) => {
                    let mut read: std::collections::HashMap<String, Slot> =
                        std::collections::HashMap::new();
                    // A `null` value is a slot the intent declares and nothing
                    // has filled: the deserializer reads it as a slot with no
                    // value, which is how this crate holds an unfilled one.
                    reader
                        .read_map(member, &mut |key, value| {
                            let slot = <Slot as ::aws_smithy_schema_server::DeserializableStruct>
                                ::deserialize(value)
                                .map_err(|e| {
                                    ::aws_smithy_schema_server::within_map_entry(e, &key)
                                })?;
                            read.insert(key, slot);
                            Ok(())
                        })
                        .map_err(|e| ::aws_smithy_schema_server::within_member(e, "slots"))?;
                    slots = Some(read);
                }
                Some(MEMBER_INDEX_STATE) => {
                    state = Some(
                        reader
                            .read_string(member)
                            .map_err(|e| ::aws_smithy_schema_server::within_member(e, "state"))?,
                    );
                }
                Some(MEMBER_INDEX_CONFIRMATION_STATE) => {
                    confirmation_state = Some(reader.read_string(member).map_err(|e| {
                        ::aws_smithy_schema_server::within_member(e, "confirmationState")
                    })?);
                }
                _ => {}
            }
            Ok(())
        })?;

        Ok(Self {
            name: name
                .ok_or_else(|| ::aws_smithy_schema_server::missing_member("name"))?,
            slots,
            state,
            confirmation_state,
        })
    }
}
