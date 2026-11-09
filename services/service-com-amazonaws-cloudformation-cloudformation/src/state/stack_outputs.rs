// SPDX-License-Identifier: CC0-1.0

//! Reading ONE OUTPUT of ANOTHER STACK of the same account and Region, which is what the
//! `Fn::GetStackOutput` function this service resolves for itself answers.
//!
//! ## The rule this module implements
//!
//! A template's value functions are resolved by CloudFormation itself, and the roster of
//! them the service advertises as supported is what a template may use
//! ([`super::template_validation`] refuses anything else by name). `Fn::GetStackOutput` is
//! one of that roster: its argument is `[stack, output]`, and its value is the value that
//! output carries on the stack named — in the account and Region the deployment runs in.
//!
//! Two things separate it from `Fn::ImportValue`, and both are what this module is for:
//!
//! * it reads an output by the STACK and OUTPUT KEY rather than by an export name, so the
//!   output it reads needs no `Export` declaration at all — a stack can read any output of
//!   any other stack of its Region;
//! * the value is read WHERE THE TEMPLATE IS DEPLOYED and then kept by the reading stack
//!   ([`super::StoredStack::stack_outputs`]), so a later deployment of the stack that
//!   publishes the output does not change what the reading stack already resolved: the
//!   reader holds the value as of its own deployment until its own next one. An
//!   `Fn::ImportValue` is the other relation — an export is PINNED while a stack imports
//!   it ([`super::exports`]), because an import is a live reference — and nothing pins an
//!   output read this way, which is why the value has to be kept rather than re-read.
//!
//! A reference naming a stack this Region does not hold, or an output that stack does not
//! carry, has NO value: the deployment cannot be carried out, and the stack reports why
//! ([`no_output_named`]) rather than deploying a resource configured with a blank — which
//! is the same answer an `Fn::ImportValue` naming an export nothing publishes gets
//! ([`super::no_export_named`]).
//!
//! ## Bounds
//!
//! The table one deployment resolves against is this Region's live stacks and their
//! outputs, both already bounded by the store itself ([`super::MAX_STACKS`] stacks, and a
//! template's own output quota per stack), and the references one template reads are
//! collected under the deployment's shared evaluation budget
//! ([`super::template::Budget`]), like every other pass over a caller-sized body.

use std::collections::BTreeMap;

/// The function a template reads another stack's output with, as a template spells it.
pub const FN_GET_STACK_OUTPUT: &str = "Fn::GetStackOutput";

/// The member of an `Fn::GetStackOutput` argument that names the stack to read, as the
/// function's requests spell it: `{"Fn::GetStackOutput": {"StackName": <stack>,
/// "OutputName": <output>}}`.
pub const STACK_NAME: &str = "StackName";

/// The member that names the output to read. See [`STACK_NAME`].
pub const OUTPUT_NAME: &str = "OutputName";

/// One output of one stack, as a template names the pair.
///
/// `stack` is the stack as the reference SPELLS it — a stack is addressable by name and by
/// id, so [`values_in`] carries both spellings — and `output` is the `OutputKey` the
/// reference reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub stack: String,
    pub output: String,
}

impl Reference {
    /// The reference to `output` of `stack`.
    pub fn new(stack: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            stack: stack.into(),
            output: output.into(),
        }
    }
}

/// What an `Fn::GetStackOutput` resolves against: an output value per reference that reads
/// one.
///
/// A deployment resolves against the whole table of its Region ([`values_in`]); the stack it
/// deploys then KEEPS the entries its template really read
/// ([`super::StoredStack::stack_outputs`]), and every later evaluation of that stack's own
/// template resolves against those — which is what makes the value the reader holds its
/// own deployment's rather than the publishing stack's latest.
pub type Values = BTreeMap<Reference, String>;

/// Every output value a template deployed in one Region can read, keyed by the reference
/// that reads it.
///
/// One entry per output of each live stack of that Region that this service evaluated a
/// value for, under BOTH spellings the stack is addressable by (its name and its id), since
/// a reference may name either. An output whose value this service could not evaluate
/// carries none, so it is not readable — the same reason it is not exportable.
pub fn values_in<'a>(stacks: impl Iterator<Item = &'a super::StoredStack>) -> Values {
    let mut values = Values::new();
    for stack in stacks {
        for output in &stack.outputs {
            let Some(value) = &output.value else {
                continue;
            };
            values.insert(
                Reference::new(stack.stack_name.as_str(), output.key.as_str()),
                value.clone(),
            );
            values.insert(
                Reference::new(stack.stack_id.as_str(), output.key.as_str()),
                value.clone(),
            );
        }
    }
    values
}

/// The reason CloudFormation reports a DEPLOYMENT it could not carry out because the
/// template reads an output no stack of its account and Region carries.
///
/// The counterpart of [`super::no_export_named`], and reported the same way: the function
/// has no value to return, so the deployment fails with this service's own diagnostic
/// before anything is asked of the service that owns any resource. The output key and the
/// stack are the ones the template named and the rest is fixed text.
pub fn no_output_named(reference: &Reference) -> String {
    format!(
        "No output named {} found on stack {}.",
        reference.output, reference.stack
    )
}
