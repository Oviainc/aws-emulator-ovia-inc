// SPDX-License-Identifier: CC0-1.0

//! How this service puts a refusal it decided against a STORED execution on the wire.
//!
//! A device-facing refusal that the service reaches by comparing the request with the
//! job execution it holds — a status the execution's own state forbids, or an
//! `expectedVersion` that is not the version stored — reports that execution's current
//! state in the error body beside the `message`, as the `executionState` object
//! (`status`, `statusDetails`, `versionNumber`). The service's own documentation states
//! it for both: `InvalidStateTransitionException` is "an update attempted to change the
//! job execution to a state that is invalid because of the job execution's current state
//! … In this case, the body of the error message also contains the executionState
//! field", and a mismatched `expectedVersion` is rejected with "an ErrorResponse that
//! contains the current job execution status data … (This makes it unnecessary to perform
//! a separate DescribeJobExecution request in order to obtain the job execution status
//! data.)".
//!
//! The member is NOT in the error shapes the Smithy model declares (each models
//! `message` alone), so the generated struct cannot carry it and this module writes it:
//! [`RefusalDocument`] wraps a modelled error struct, delegates the members the model
//! does declare to that struct's generated serializer, and then writes the
//! `executionState` member of the error shape it wraps. That keeps one copy of the rule
//! for every error shape it holds for, exactly as `super::response_document` keeps one
//! copy of the success-document rule.
//!
//! A refusal decided from the REQUEST alone (a `stepTimeoutInMinutes` outside its
//! documented range, a `statusDetails` map the execution cannot hold) reports no state:
//! there is no state it was refused against, so it is built with
//! [`RefusalDocument::message_only`] and reaches the wire exactly as the model declares
//! it.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

use super::{InvalidRequestException, InvalidStateTransitionException, JobExecutionState};

/// The namespace every shape of this service is declared in, so a member written here
/// carries the shape id of the error body it belongs to.
const NAMESPACE: &str = "com.amazonaws.iotjobsdataplane";

/// The index of the `executionState` member in the body it is written into: each of
/// these error shapes declares exactly one member (`message`, index 0), so the state
/// follows it.
const EXECUTION_STATE_INDEX: usize = 1;

/// An error shape of this service whose body reports the job execution the refusal was
/// decided against.
///
/// The associated member schema is what names the object on the wire, so each error
/// shape carries its own: a member belongs to the structure that holds it.
pub trait ReportsExecutionState {
    /// The `executionState` member of THIS error shape's body.
    const EXECUTION_STATE: &'static Schema;
}

/// `InvalidStateTransitionException$executionState` — the state the refused transition
/// was decided against.
static INVALID_STATE_TRANSITION_EXECUTION_STATE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.iotjobsdataplane#InvalidStateTransitionException$executionState",
        NAMESPACE,
        "executionState",
    ),
    ShapeType::Structure,
    "executionState",
    EXECUTION_STATE_INDEX,
);

impl ReportsExecutionState for InvalidStateTransitionException {
    const EXECUTION_STATE: &'static Schema = &INVALID_STATE_TRANSITION_EXECUTION_STATE;
}

/// `InvalidRequestException$executionState` — the "current job execution status data" a
/// mismatched `expectedVersion` is refused with.
static INVALID_REQUEST_EXECUTION_STATE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.iotjobsdataplane#InvalidRequestException$executionState",
        NAMESPACE,
        "executionState",
    ),
    ShapeType::Structure,
    "executionState",
    EXECUTION_STATE_INDEX,
);

impl ReportsExecutionState for InvalidRequestException {
    const EXECUTION_STATE: &'static Schema = &INVALID_REQUEST_EXECUTION_STATE;
}

/// A modelled error `E` as this service writes its body: the members the model declares,
/// plus the execution's current state when the refusal was decided against one.
///
/// Built in a handler from the record the refusal was read from:
///
/// ```ignore
///     RefusalDocument::with_execution_state(
///         InvalidStateTransitionException { message: Some(reason) },
///         crate::state::to_execution_state(record),
///     )
/// ```
#[derive(Debug, Clone)]
pub struct RefusalDocument<E> {
    error: E,
    execution_state: Option<JobExecutionState>,
}

impl<E> RefusalDocument<E> {
    /// The refusal `error`, carrying the state of the execution it was decided against.
    pub fn with_execution_state(error: E, execution_state: JobExecutionState) -> Self {
        Self {
            error,
            execution_state: Some(execution_state),
        }
    }

    /// The refusal `error` on its own, for one decided from the request alone: there is
    /// no stored state it was refused against, so the body is the one the model declares.
    pub fn message_only(error: E) -> Self {
        Self {
            error,
            execution_state: None,
        }
    }
}

impl<E: ::std::fmt::Display> ::std::fmt::Display for RefusalDocument<E> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        self.error.fmt(f)
    }
}

impl<E: ::std::fmt::Debug + ::std::fmt::Display> ::std::error::Error for RefusalDocument<E> {}

impl<E: SerializableStruct + ReportsExecutionState> SerializableStruct for RefusalDocument<E> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        // The modelled members first, written by the generated serializer of the shape
        // itself, so the body cannot drift from the model.
        self.error.serialize_members(serializer)?;
        if let Some(execution_state) = self.execution_state.as_ref() {
            serializer.write_struct(E::EXECUTION_STATE, execution_state)?;
        }
        Ok(())
    }
}
