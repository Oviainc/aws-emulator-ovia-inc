// SPDX-License-Identifier: CC0-1.0

//! The projection EMR Serverless writes for an application or a session on a READ: the
//! resource's EFFECTIVE configuration, defaults and all.
//!
//! A read of an application or of an interactive session reports the configuration the
//! service applied to it, not the subset the caller happened to name: every
//! configuration block is present, each carrying the value it was created with or the
//! documented default the service filled in, and the service's own wire response carries
//! a few keys for that projection that this service's Smithy model does not declare at
//! all — the auto-start and auto-stop blocks under a second, shorter spelling
//! (`autoStartConfig`, `autoStopConfig`), the auto-stop idle timeout under a second name
//! inside each of them (`idleTimeout` beside `idleTimeoutMinutes`), the application's
//! extended-support projection, and the extended-support flag of a session. A client
//! reading one of those keys cannot tell "off" from "not reported" when the key is
//! absent, which is what this module exists to prevent.
//!
//! The half of the rule that is MODELLED lives where the record is built — the service's
//! `adopt_*` functions fill each documented default in at create and update time, so
//! every reader of the stored record reports the same values. The half that is not
//! modelled is here, because a member no model declares cannot be a field of a generated
//! shape: it is written by the SERIALIZER every modelled response of this service is
//! rendered through ([`smithy_server_runtime::Service::override_protocol`], wired in
//! `super`), from the value the handler already returned rather than from the request —
//! so the two spellings of one setting can never disagree, and nothing is keyed on which
//! operation was called.
//!
//! [`ReadProjection`] is that wrapper: every write delegates to the serializer the
//! framework's protocol built, unchanged, except where the member being written is one
//! the projection extends. A struct member named `application` or `session` is descended
//! into with the projection applied one level down; inside an application, the auto-start
//! and auto-stop blocks are written a second time under their other spelling and the
//! extended-support block is appended; inside an auto-stop block, the idle timeout is
//! written a second time under its other name; inside a session, the extended-support
//! flag is appended. Every other member, at every depth, is the framework's own bytes.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The namespace the synthetic member schemas below sit in: this service's own, since
/// each one is a member of one of ITS shapes that its model does not declare. Nothing
/// collides — no member of these names exists in the model — and the ids never leave the
/// process: the JSON codec writes a member's NAME as the object key.
const NAMESPACE: &str = "com.amazonaws.emrserverless";

/// The output member that carries an `Application` (`GetApplication`,
/// `UpdateApplication`) — `smithy_query(query="operation",operation="GetApplication")`.
const APPLICATION_MEMBER: &str = "application";

/// The output member that carries a `Session` (`GetSession`).
const SESSION_MEMBER: &str = "session";

/// `Application$autoStartConfiguration`, the modelled spelling of the auto-start block.
const AUTO_START_CONFIGURATION_MEMBER: &str = "autoStartConfiguration";

/// `Application$autoStopConfiguration`, the modelled spelling of the auto-stop block.
const AUTO_STOP_CONFIGURATION_MEMBER: &str = "autoStopConfiguration";

/// `AutoStopConfig$idleTimeoutMinutes`, the modelled spelling of the idle timeout.
const IDLE_TIMEOUT_MINUTES_MEMBER: &str = "idleTimeoutMinutes";

/// The index a synthetic member schema carries.
///
/// A member's index orders the slots the DESERIALIZE path fills; these schemas are only
/// ever written, and the JSON writer reads a member's name alone, so one index serves
/// them all.
const SYNTHETIC_INDEX: usize = 0;

/// The auto-start block's other spelling, beside the modelled one.
static M_AUTO_START_CONFIG: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#Application$autoStartConfig",
        NAMESPACE,
        "autoStartConfig",
    ),
    ShapeType::Structure,
    "autoStartConfig",
    SYNTHETIC_INDEX,
);

/// The auto-stop block's other spelling, beside the modelled one.
static M_AUTO_STOP_CONFIG: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#Application$autoStopConfig",
        NAMESPACE,
        "autoStopConfig",
    ),
    ShapeType::Structure,
    "autoStopConfig",
    SYNTHETIC_INDEX,
);

/// The idle timeout's other name, inside either spelling of the auto-stop block. It is
/// the same number of minutes as the modelled member: a caller reading either one waits
/// out the same span.
static M_IDLE_TIMEOUT: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#AutoStopConfig$idleTimeout",
        NAMESPACE,
        "idleTimeout",
    ),
    ShapeType::Integer,
    "idleTimeout",
    SYNTHETIC_INDEX,
);

/// The application's extended-support projection.
static M_EXTENDED_SUPPORT_CONFIGURATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#Application$extendedSupportConfiguration",
        NAMESPACE,
        "extendedSupportConfiguration",
    ),
    ShapeType::Structure,
    "extendedSupportConfiguration",
    SYNTHETIC_INDEX,
);

/// Whether the application runs under extended support.
static M_EXTENDED_SUPPORT_ENABLED: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#ExtendedSupportConfiguration$enabled",
        NAMESPACE,
        "enabled",
    ),
    ShapeType::Boolean,
    "enabled",
    SYNTHETIC_INDEX,
);

/// Which level the extended-support setting in force was set at.
static M_EXTENDED_SUPPORT_SOURCE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#ExtendedSupportConfiguration$source",
        NAMESPACE,
        "source",
    ),
    ShapeType::String,
    "source",
    SYNTHETIC_INDEX,
);

/// When the extended-support setting was last changed, and by whom: reported as the
/// absent values they are while nothing has changed the account's setting.
static M_EXTENDED_SUPPORT_UPDATED_AT: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#ExtendedSupportConfiguration$updatedAt",
        NAMESPACE,
        "updatedAt",
    ),
    ShapeType::Timestamp,
    "updatedAt",
    SYNTHETIC_INDEX,
);

static M_EXTENDED_SUPPORT_UPDATED_BY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#ExtendedSupportConfiguration$updatedBy",
        NAMESPACE,
        "updatedBy",
    ),
    ShapeType::String,
    "updatedBy",
    SYNTHETIC_INDEX,
);

/// The extended-support flag of one interactive session.
static M_SESSION_EXTENDED_SUPPORT_ENABLED: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.emrserverless#Session$extendedSupportEnabled",
        NAMESPACE,
        "extendedSupportEnabled",
    ),
    ShapeType::Boolean,
    "extendedSupportEnabled",
    SYNTHETIC_INDEX,
);

/// Which shape's members are being written, and therefore what the projection adds to
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// An operation's output structure: the projection reaches whatever resource its
    /// members carry, and adds nothing of its own.
    Output,
    /// One `Application`.
    Application,
    /// One `Session`.
    Session,
    /// One `AutoStopConfig`, inside either of its spellings.
    AutoStop,
}

/// One structure, serialized with the projection its [`Level`] states applied.
///
/// This is what the service's protocol hands the framework's `serialize_output` in place
/// of the output it was given, so the status, the headers and every modelled member stay
/// exactly what the framework rendered.
pub struct ReadProjection<'a> {
    inner: &'a dyn SerializableStruct,
    level: Level,
}

impl<'a> ReadProjection<'a> {
    /// Wrap the output structure a modelled response is about to be rendered from.
    pub fn new(inner: &'a dyn SerializableStruct) -> Self {
        Self {
            inner,
            level: Level::Output,
        }
    }

    /// Wrap a structure NESTED in that output, at the level the projection reached it.
    fn at(inner: &'a dyn SerializableStruct, level: Level) -> Self {
        Self { inner, level }
    }
}

impl SerializableStruct for ReadProjection<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        // The modelled members first, written by the generated code through the
        // projecting serializer below — which is where a member that gains a second
        // spelling gets it, in the position the framework wrote the first one.
        let idle_timeout_minutes = {
            let mut projecting = Projecting {
                inner: &mut *s,
                level: self.level,
                idle_timeout_minutes: None,
            };
            self.inner.serialize_members(&mut projecting)?;
            projecting.idle_timeout_minutes
        };

        // Then what this level adds after them.
        match self.level {
            Level::Output => Ok(()),
            Level::Application => {
                s.write_struct(&M_EXTENDED_SUPPORT_CONFIGURATION, &ExtendedSupport)
            }
            Level::Session => s.write_boolean(
                &M_SESSION_EXTENDED_SUPPORT_ENABLED,
                crate::state::EXTENDED_SUPPORT_ENABLED,
            ),
            // The timeout under its other name, and only when the block reported one:
            // the projection restates the value the record holds, never invents one.
            Level::AutoStop => match idle_timeout_minutes {
                Some(minutes) => s.write_integer(&M_IDLE_TIMEOUT, minutes),
                None => Ok(()),
            },
        }
    }
}

/// The extended-support projection of an application: the setting in force, the level it
/// was set at, and when and by whom it was last changed.
///
/// Read from [`crate::state`], which is where this service's configuration defaults and
/// their grounding live.
struct ExtendedSupport;

impl SerializableStruct for ExtendedSupport {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_boolean(
            &M_EXTENDED_SUPPORT_ENABLED,
            crate::state::EXTENDED_SUPPORT_ENABLED,
        )?;
        s.write_string(
            &M_EXTENDED_SUPPORT_SOURCE,
            crate::state::EXTENDED_SUPPORT_SOURCE,
        )?;
        s.write_null(&M_EXTENDED_SUPPORT_UPDATED_AT)?;
        s.write_null(&M_EXTENDED_SUPPORT_UPDATED_BY)
    }
}

/// The serializer the members of one structure are written through: the framework's,
/// with the members this projection extends written twice and the values it restates
/// captured on the way past.
struct Projecting<'a> {
    inner: &'a mut dyn ShapeSerializer,
    level: Level,
    /// The idle timeout an `AutoStopConfig` reported, so it can be restated under its
    /// other name once the block's modelled members are written.
    idle_timeout_minutes: Option<i32>,
}

impl ShapeSerializer for Projecting<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        match (self.level, schema.member_name()) {
            // Descend into the resource an output carries, so the rest of this
            // projection applies to the resource rather than to the output.
            (Level::Output, Some(APPLICATION_MEMBER)) => self
                .inner
                .write_struct(schema, &ReadProjection::at(value, Level::Application)),
            (Level::Output, Some(SESSION_MEMBER)) => self
                .inner
                .write_struct(schema, &ReadProjection::at(value, Level::Session)),
            // One setting, under both of the spellings the service's response carries:
            // the same value written twice, so the two can never disagree.
            (Level::Application, Some(AUTO_START_CONFIGURATION_MEMBER)) => {
                self.inner.write_struct(schema, value)?;
                self.inner.write_struct(&M_AUTO_START_CONFIG, value)
            }
            (Level::Application, Some(AUTO_STOP_CONFIGURATION_MEMBER)) => {
                let projected = ReadProjection::at(value, Level::AutoStop);
                self.inner.write_struct(schema, &projected)?;
                self.inner.write_struct(&M_AUTO_STOP_CONFIG, &projected)
            }
            _ => self.inner.write_struct(schema, value),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, write_entries)
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        if self.level == Level::AutoStop && schema.member_name() == Some(IDLE_TIMEOUT_MINUTES_MEMBER)
        {
            self.idle_timeout_minutes = Some(value);
        }
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_float(schema, value)
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}
