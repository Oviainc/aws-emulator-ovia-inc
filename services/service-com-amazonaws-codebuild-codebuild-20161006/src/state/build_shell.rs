// SPDX-License-Identifier: CC0-1.0

//! The build environment's SHELL: what ONE command line of a buildspec does.
//!
//! # The rule this module implements
//!
//! A build's phase commands are command LINES, which CodeBuild hands to the shell of
//! the build environment; what the build reports for a phase is the status that shell
//! answered with — zero for a command that ran to success, non-zero otherwise — so the
//! outcome of a build follows from running its own commands and from nothing else.
//!
//! The grammar is not read here: a line is tokenized and parsed by `brush-parser`, the
//! POSIX/bash tokenizer and grammar, exactly as this workspace reads caller-supplied SQL
//! with `sqlparser` and caller-supplied YAML with `yaml-rust2`. That parser builds a
//! syntax tree and runs nothing, so the and-or lists, pipelines, assignments and
//! quoting of a command line are the shell's own rather than something scanned out of
//! the text. Word expansion is read from the same parser's word pieces.
//!
//! # Why the grammar and not an engine
//!
//! The one maintained shell ENGINE this grammar has — `brush-core`, the executing half of
//! the same project — is deliberately not a dependency of this crate, and what follows is
//! not a reimplementation of it. An engine's whole purpose is to run what the shell
//! language can express: it resolves a command name against the host's `PATH` and starts
//! a host PROCESS for it, opens the host files a redirection names, and expands a command
//! substitution by running that command. Those are exactly the primitives this module has
//! none of, and the crate exposes no seam to replace them with an in-memory volume or an
//! in-process dispatcher — its command resolution and its redirections go straight to the
//! host through the standard library — so there is no configuration of it that could be
//! confined here: taking it would hand a caller's buildspec the developer's machine.
//! Nor would it buy fidelity that is otherwise reachable, because what a real buildspec's
//! commands need is the programs of a build CONTAINER, which this platform does not run
//! at all.
//!
//! What this module does instead is narrower than a shell by construction, and it is not
//! an implementation of the shell LANGUAGE: there is no evaluator for the language's
//! constructs here, and none is approximated. It PERFORMS a fixed catalog of
//! session-local utilities over the real grammar's tree ([`BUILT_IN_COMMANDS`]), and
//! every other command, and every command FORM and expansion it does not perform, fails
//! closed with the status a shell answers for a command it cannot run. There is therefore
//! no language subset to keep in step with a specification: a command either names one of
//! the seven utilities, or it did not run.
//!
//! # What this build environment provides
//!
//! It runs IN THIS PROCESS. It therefore has no process to start, no network to reach
//! and no working tree, and the utilities it provides are the ones whose whole effect is
//! inside the build's own session: [`BUILT_IN_COMMANDS`]. Every other command is
//! reported the way a shell reports a command it has no program for — status
//! [`EXIT_COMMAND_NOT_FOUND`] — so a build that depends on one FAILS instead of being
//! reported as having run it. Nothing here starts a process, opens a connection or
//! touches a file.
//!
//! What a performed command contributes to the build is its exit status and the
//! variables it sets (a buildspec's `env/exported-variables` are read back off this
//! session). The bytes a command writes to its output are not delivered anywhere yet —
//! a build's log group is its own concern — so this shell keeps none of them.

use std::collections::BTreeMap;

/// The exit status of a command that ran to success.
const EXIT_SUCCESS: i32 = 0;

/// The exit status of a command that ran and answered failure, which is what the
/// `false` utility is specified to do ("exit with a status code indicating failure").
const EXIT_FAILURE: i32 = 1;

/// The exit status a shell reports for a command line it could not parse — the status
/// `sh` is specified to exit with on a syntax error it detects.
const EXIT_SYNTAX_ERROR: i32 = 2;

/// The exit status a shell reports when "the utility to be executed could not be
/// found", which is what this environment answers for every command it provides no
/// program for, and for a command form it does not perform.
pub(super) const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// The commands this build environment provides, as their names.
///
/// Each one's whole effect is inside the build's own session, which is why it can be
/// performed in process: `echo` writes to the command's output, `true`, `false` and `:`
/// answer a status, `exit` ends the build's session, and `export`/`unset` change the
/// session's variables. A command that is not one of these needs a program, a working
/// tree or a network this environment does not have.
pub(super) const BUILT_IN_COMMANDS: &[&str] =
    &["echo", "true", "false", ":", "exit", "export", "unset"];

/// The commands an assignment written before them persists in the session for: POSIX's
/// special built-in utilities, for which "variable assignments specified with special
/// built-in utilities remain in effect after the built-in completes". For every other
/// command an assignment before it is that command's environment alone.
const PERSISTING_COMMANDS: &[&str] = &[":", "exit", "export", "unset"];

/// How many commands one build's session runs before it is given up on. THIS NUMBER IS
/// THE EMULATOR'S: a buildspec is caller-supplied text, so the work one build can cost
/// is bounded here rather than by the model, which puts no ceiling on a buildspec's
/// size. It is far above any real buildspec's command count.
pub(super) const MAX_COMMANDS_PER_BUILD: usize = 1_000;

/// The longest command line this session reads. THIS NUMBER IS THE EMULATOR'S, for the
/// same reason as [`MAX_COMMANDS_PER_BUILD`]: it bounds what one line can cost the
/// parser, and is far above any real build command.
pub(super) const MAX_COMMAND_BYTES: usize = 16 * 1024;

/// How many grouping indicators one command line may carry before this session gives up
/// on reading it. THIS NUMBER IS THE EMULATOR'S, and it is what keeps the grammar's own
/// recursion in range: a parser descends once per nested group, so a line nested
/// arbitrarily deep would exhaust the stack of the thread serving the request. It is far
/// above any real build command, and it is checked WITHOUT parsing — the same shape
/// [`super::batchspec`] bounds a buildspec's nesting by.
const MAX_COMMAND_NESTING: usize = 64;

/// The characters that open a nested group in a command line, counted by
/// [`within_parse_bounds`]: a subshell or command substitution, a brace group or
/// expansion, a bracket expression, and the older command-substitution spelling.
const NESTING_INDICATORS: [char; 4] = ['(', '{', '[', '`'];

/// The reserved WORDS that open or close a nested command in a command line, counted by
/// [`within_parse_bounds`] beside [`NESTING_INDICATORS`]: the shell's compound commands
/// nest through these rather than through any character, so counting characters alone
/// would leave the grammar's recursion unbounded for a line of `if … then …`. The words
/// that CLOSE one are counted too, which can only make the count higher — an over-count
/// refuses a line and never admits one.
const NESTING_WORDS: [&str; 16] = [
    "if", "then", "elif", "else", "fi", "while", "until", "for", "select", "case", "esac", "in",
    "do", "done", "function", "time",
];

/// The characters a reserved word may be glued to in a command line: the shell's own
/// separators and operators. A word is counted by [`within_parse_bounds`] when it stands
/// between any of these (or whitespace), so `:;if` carries an `if` exactly as `: ; if`
/// does.
const WORD_SEPARATORS: [char; 10] = [';', '&', '|', '(', ')', '{', '}', '<', '>', '!'];

/// Whether `line` is inside the bounds the parse below is given: its size, and how many
/// nested commands it opens — through a grouping character or a reserved word.
fn within_parse_bounds(line: &str) -> bool {
    if line.len() > MAX_COMMAND_BYTES {
        return false;
    }
    let grouped = line
        .chars()
        .filter(|character| NESTING_INDICATORS.contains(character))
        .count();
    let worded = line
        .split(|character: char| {
            character.is_whitespace() || WORD_SEPARATORS.contains(&character)
        })
        .filter(|word| NESTING_WORDS.contains(word))
        .count();
    grouped.saturating_add(worded) <= MAX_COMMAND_NESTING
}

/// How many bytes one expansion may produce before this session gives up on the word.
/// THIS NUMBER IS THE EMULATOR'S: a parameter expansion copies a value this session
/// holds, so a word that expands several of them is how one command line could grow what
/// the next one copies again. It is far above any real build command's arguments.
const MAX_EXPANSION_BYTES: usize = 64 * 1024;

/// How many bytes of variables one build's session may hold at once. THIS NUMBER IS THE
/// EMULATOR'S, under [`MAX_EXPANSION_BYTES`]' rule applied to the session rather than to
/// one word: past it a command that would set another variable is not performed, so the
/// store one buildspec can grow is bounded whatever its commands assign.
const MAX_SESSION_BYTES: usize = 1024 * 1024;

/// How many variables one build's session may hold at once. THIS NUMBER IS THE
/// EMULATOR'S: [`MAX_SESSION_BYTES`] bounds the store's SIZE and this bounds its
/// cardinality, so neither a few long values nor very many short ones can grow it
/// without a stated ceiling. Far above any real buildspec's variables.
const MAX_SESSION_VARIABLES: usize = 4096;

/// How many bytes ONE command line's expansions may materialise at once. THIS NUMBER IS
/// THE EMULATOR'S: [`MAX_EXPANSION_BYTES`] bounds one word, and this bounds their SUM,
/// because a line may carry many words and each one's expansion is live at the same
/// instant. Past it the command is not performed.
const MAX_LINE_EXPANSION_BYTES: usize = 256 * 1024;

/// How deeply a word's expansion is walked before this session gives up on it. THIS
/// NUMBER IS THE EMULATOR'S, under [`MAX_COMMAND_NESTING`]'s rule, applied to the pieces
/// a quoted word nests.
const MAX_WORD_DEPTH: usize = 32;

/// The session one build's commands run in: the variables they see and change, and how
/// much running is left in the build's budget.
#[derive(Debug)]
pub(super) struct BuildShell {
    /// The variables this session holds, which is what a word's parameter expansion
    /// resolves against and what the buildspec's exported variables are read from.
    variables: BTreeMap<String, String>,
    /// How many bytes those variables' names and values come to, kept as they are
    /// written rather than recomputed, so the [`MAX_SESSION_BYTES`] check costs one
    /// addition however many variables the session holds.
    held_bytes: usize,
    /// How many more bytes the command line being run may expand to
    /// ([`MAX_LINE_EXPANSION_BYTES`]), reset for each line.
    expansion_left: usize,
    /// How many more commands this build may run ([`MAX_COMMANDS_PER_BUILD`]).
    commands_left: usize,
    /// The status `exit` ended this session with, once a command asked for one: every
    /// later command of the build is then not run at all, the way a shell stops at
    /// `exit`.
    exit_status: Option<i32>,
}

impl BuildShell {
    /// A session holding `variables`, with the build's whole command budget left.
    pub(super) fn new(variables: BTreeMap<String, String>) -> Self {
        let held_bytes = variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum();
        Self {
            variables,
            held_bytes,
            expansion_left: MAX_LINE_EXPANSION_BYTES,
            commands_left: MAX_COMMANDS_PER_BUILD,
            exit_status: None,
        }
    }

    /// The value this session holds for `name`, or `None` when it holds none.
    pub(super) fn variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    /// The status `exit` ended this session with, or `None` while nothing has.
    pub(super) fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Run one command LINE and answer the status the shell answered for it.
    ///
    /// A line that does not parse is the shell's own syntax error
    /// ([`EXIT_SYNTAX_ERROR`]); a line outside the bounds this parse is given
    /// ([`within_parse_bounds`]), or one reached after the build's command budget is
    /// spent, is a command this environment did not run at all, which is reported as a
    /// command it could not run rather than as one that succeeded.
    pub(super) fn run(&mut self, line: &str) -> i32 {
        if !within_parse_bounds(line) || self.commands_left == 0 {
            return EXIT_COMMAND_NOT_FOUND;
        }
        // Each line gets the whole expansion budget: the words of one line are live at
        // the same instant, and the words of the line before it are not.
        self.expansion_left = MAX_LINE_EXPANSION_BYTES;
        let Ok(tokens) = ::brush_parser::tokenize_str(line) else {
            return EXIT_SYNTAX_ERROR;
        };
        let options = ::brush_parser::ParserOptions::default();
        let Ok(program) = ::brush_parser::parse_tokens(&tokens, &options) else {
            return EXIT_SYNTAX_ERROR;
        };
        let mut status = EXIT_SUCCESS;
        for complete_command in &program.complete_commands {
            for item in &complete_command.0 {
                status = self.run_and_or_list(&item.0);
                if self.exit_status.is_some() {
                    return status;
                }
            }
        }
        status
    }

    /// Run an and-or list: its first pipeline, then each later pipeline only when the
    /// operator before it says so — `&&` runs the next on a zero status, `||` on a
    /// non-zero one — and answer the status of the last pipeline that ran.
    fn run_and_or_list(&mut self, list: &::brush_parser::ast::AndOrList) -> i32 {
        let mut status = self.run_pipeline(&list.first);
        for next in &list.additional {
            if self.exit_status.is_some() {
                break;
            }
            match next {
                ::brush_parser::ast::AndOr::And(pipeline) => {
                    if status == EXIT_SUCCESS {
                        status = self.run_pipeline(pipeline);
                    }
                }
                ::brush_parser::ast::AndOr::Or(pipeline) => {
                    if status != EXIT_SUCCESS {
                        status = self.run_pipeline(pipeline);
                    }
                }
            }
        }
        status
    }

    /// Run a pipeline and answer its status: "the exit status of a pipeline shall be the
    /// exit status of the last command", negated when the pipeline is written with `!`.
    ///
    /// Every command of the pipeline is run, in order, because each answers its own
    /// status; what one writes is read by the next, and this environment keeps no
    /// command output, so nothing is passed along but the statuses.
    fn run_pipeline(&mut self, pipeline: &::brush_parser::ast::Pipeline) -> i32 {
        let mut status = EXIT_SUCCESS;
        for command in &pipeline.seq {
            status = self.run_command(command);
            if self.exit_status.is_some() {
                break;
            }
        }
        if pipeline.bang {
            status = if status == EXIT_SUCCESS {
                EXIT_FAILURE
            } else {
                EXIT_SUCCESS
            };
        }
        status
    }

    /// Run one command of a pipeline.
    ///
    /// A SIMPLE command is the form this environment performs. Every other form a shell
    /// grammar admits — a compound command, a function definition, an extended test —
    /// is one this environment does not perform, and a command that did not run answers
    /// as one that could not be run rather than as one that succeeded.
    fn run_command(&mut self, command: &::brush_parser::ast::Command) -> i32 {
        match command {
            ::brush_parser::ast::Command::Simple(simple) => self.run_simple_command(simple),
            _ => EXIT_COMMAND_NOT_FOUND,
        }
    }

    /// Run a simple command: its variable assignments, then the command its first word
    /// names with the words after it as its arguments.
    ///
    /// A command with no word at all is the assignment form — "the variable assignments
    /// shall affect the current execution environment" — which is how a buildspec sets a
    /// variable the build later exports. An assignment written BEFORE a command word
    /// affects that command's environment alone, and persists in the session only for the
    /// special built-in utilities POSIX keeps it for ([`PERSISTING_COMMANDS`]); since
    /// every word is expanded before the assignments are applied, a command that does not
    /// keep them sees the value either way. Any prefix or suffix item that is neither a
    /// word nor an assignment is a redirection or a substitution this environment does
    /// not perform, and the command is then not run.
    fn run_simple_command(&mut self, simple: &::brush_parser::ast::SimpleCommand) -> i32 {
        self.commands_left = self.commands_left.saturating_sub(1);
        let mut assignments: Vec<(String, String)> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        let prefix = simple.prefix.as_ref().map(|prefix| &prefix.0);
        let suffix = simple.suffix.as_ref().map(|suffix| &suffix.0);
        for item in prefix.into_iter().chain(suffix).flatten() {
            match item {
                ::brush_parser::ast::CommandPrefixOrSuffixItem::Word(word) => {
                    let Some(expanded) = self.expand_word(&word.value) else {
                        return EXIT_COMMAND_NOT_FOUND;
                    };
                    words.push(expanded);
                }
                ::brush_parser::ast::CommandPrefixOrSuffixItem::AssignmentWord(
                    assignment,
                    _,
                ) => {
                    let Some(pair) = self.assignment_of(assignment) else {
                        return EXIT_COMMAND_NOT_FOUND;
                    };
                    assignments.push(pair);
                }
                _ => return EXIT_COMMAND_NOT_FOUND,
            }
        }
        let name = match simple.word_or_name.as_ref() {
            Some(word) => match self.expand_word(&word.value) {
                Some(expanded) => Some(expanded),
                None => return EXIT_COMMAND_NOT_FOUND,
            },
            None => None,
        };
        let persists = name
            .as_deref()
            .is_none_or(|name| PERSISTING_COMMANDS.contains(&name));
        if persists {
            for (name, value) in assignments {
                if !self.set_variable(name, value) {
                    return EXIT_COMMAND_NOT_FOUND;
                }
            }
        }
        match name {
            Some(name) => self.run_built_in(&name, &words),
            // Nothing but assignments: they have been applied, which is the whole
            // command.
            None => EXIT_SUCCESS,
        }
    }

    /// One assignment of a simple command as the name and value it sets, or `None` when
    /// it is a form this environment does not perform — an array assignment, an
    /// appending assignment, or a value whose expansion it cannot resolve.
    fn assignment_of(
        &mut self,
        assignment: &::brush_parser::ast::Assignment,
    ) -> Option<(String, String)> {
        if assignment.append {
            return None;
        }
        let ::brush_parser::ast::AssignmentName::VariableName(name) = &assignment.name else {
            return None;
        };
        let ::brush_parser::ast::AssignmentValue::Scalar(value) = &assignment.value else {
            return None;
        };
        Some((name.clone(), self.expand_word(&value.value)?))
    }

    /// Set `name` to `value` in this session, unless the session is already holding
    /// [`MAX_SESSION_BYTES`] or [`MAX_SESSION_VARIABLES`] — `false` is then a command
    /// this environment did not perform, which is what its caller reports.
    fn set_variable(&mut self, name: String, value: String) -> bool {
        let displaced = self
            .variables
            .get(&name)
            .map(|held| name.len().saturating_add(held.len()));
        if displaced.is_none() && self.variables.len() >= MAX_SESSION_VARIABLES {
            return false;
        }
        let would_hold = self
            .held_bytes
            .saturating_sub(displaced.unwrap_or(0))
            .saturating_add(name.len())
            .saturating_add(value.len());
        if would_hold > MAX_SESSION_BYTES {
            return false;
        }
        self.held_bytes = would_hold;
        self.variables.insert(name, value);
        true
    }

    /// Remove `name` from this session, with the bytes it held.
    fn unset_variable(&mut self, name: &str) {
        if let Some(value) = self.variables.remove(name) {
            self.held_bytes = self
                .held_bytes
                .saturating_sub(name.len().saturating_add(value.len()));
        }
    }

    /// Run the command `name` with `arguments`, when it is one this environment provides
    /// ([`BUILT_IN_COMMANDS`]).
    ///
    /// `echo` answers success: what it writes is its output, which this environment does
    /// not deliver. `true` and `:` answer success and `false` answers failure, which is
    /// the whole of what those utilities do. `exit` ends the build's session with the
    /// status its argument names, defaulting to success, and a status outside 0..=255 is
    /// taken modulo 256 the way a shell takes an exit status. `export` sets the
    /// variables its `NAME=VALUE` arguments name and `unset` removes them.
    fn run_built_in(&mut self, name: &str, arguments: &[String]) -> i32 {
        // The catalog is what this environment provides: a command outside it needs a
        // program, a working tree or a network this environment does not have.
        if !BUILT_IN_COMMANDS.contains(&name) {
            return EXIT_COMMAND_NOT_FOUND;
        }
        match name {
            "echo" => EXIT_SUCCESS,
            "true" | ":" => EXIT_SUCCESS,
            "false" => EXIT_FAILURE,
            "exit" => {
                let status = match arguments.first() {
                    Some(argument) => match argument.parse::<i64>() {
                        Ok(status) => i32::try_from(status.rem_euclid(256)).unwrap_or(EXIT_FAILURE),
                        // A status a shell cannot read is its own usage failure, and the
                        // session still ends.
                        Err(_) => EXIT_FAILURE,
                    },
                    None => EXIT_SUCCESS,
                };
                self.exit_status = Some(status);
                status
            }
            "export" => {
                for argument in arguments {
                    // `export NAME=VALUE` sets it; `export NAME` marks a variable this
                    // session already holds, and marking one it does not hold sets
                    // nothing.
                    if let Some((name, value)) = argument.split_once('=') {
                        if !self.set_variable(name.to_string(), value.to_string()) {
                            return EXIT_COMMAND_NOT_FOUND;
                        }
                    }
                }
                EXIT_SUCCESS
            }
            "unset" => {
                for argument in arguments {
                    self.unset_variable(argument);
                }
                EXIT_SUCCESS
            }
            _ => EXIT_COMMAND_NOT_FOUND,
        }
    }

    /// `word` with its expansions resolved, or `None` when it carries an expansion this
    /// environment cannot resolve.
    ///
    /// The pieces come from `brush-parser`'s word parser, so what is quoted, what is a
    /// parameter expansion and what is literal text is the shell's own reading of the
    /// word. A parameter this session holds nothing for expands to nothing, which is
    /// what an unset parameter expands to. A command substitution, an arithmetic
    /// expansion or a tilde prefix is an expansion whose value this environment cannot
    /// resolve — it would have to run a command, or resolve a home directory it does not
    /// have — so the word has no expansion here and the command carrying it is not run.
    /// Nor does a word that expands past [`MAX_EXPANSION_BYTES`], or one whose expansion
    /// would take the line past [`MAX_LINE_EXPANSION_BYTES`].
    fn expand_word(&mut self, word: &str) -> Option<String> {
        let options = ::brush_parser::ParserOptions::default();
        let pieces = ::brush_parser::word::parse(word, &options).ok()?;
        let expanded = self.expand_pieces(&pieces, MAX_WORD_DEPTH)?;
        self.expansion_left = self.expansion_left.checked_sub(expanded.len())?;
        Some(expanded)
    }

    /// [`Self::expand_word`]'s walk over the pieces a word parsed into, with `depth`
    /// levels of quoting left: a word nested past [`MAX_WORD_DEPTH`] has no expansion
    /// here, so this walk's own recursion stays in range whatever the word nests.
    fn expand_pieces(
        &self,
        pieces: &[::brush_parser::word::WordPieceWithSource],
        depth: usize,
    ) -> Option<String> {
        let depth = depth.checked_sub(1)?;
        let mut expanded = String::new();
        for piece in pieces {
            let addition = match &piece.piece {
                ::brush_parser::word::WordPiece::Text(text)
                | ::brush_parser::word::WordPiece::SingleQuotedText(text) => text.clone(),
                ::brush_parser::word::WordPiece::DoubleQuotedSequence(inner) => {
                    self.expand_pieces(inner, depth)?
                }
                ::brush_parser::word::WordPiece::ParameterExpansion(parameter) => {
                    self.expand_parameter(parameter)?
                }
                _ => return None,
            };
            if expanded.len().saturating_add(addition.len()) > MAX_EXPANSION_BYTES {
                return None;
            }
            expanded.push_str(&addition);
        }
        Some(expanded)
    }

    /// The value a parameter expansion resolves to in this session.
    ///
    /// The plain form (`$NAME`, `${NAME}`) is the variable's value, empty when the
    /// session holds none, and the default-value form (`${NAME:-WORD}`) is that value or
    /// the default the expansion names. Every other form — a substring, a replacement, a
    /// length, an indirection, a positional or special parameter — is one this
    /// environment does not resolve.
    fn expand_parameter(
        &self,
        parameter: &::brush_parser::word::ParameterExpr,
    ) -> Option<String> {
        match parameter {
            ::brush_parser::word::ParameterExpr::Parameter {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
            } => Some(self.variable(name).unwrap_or_default().to_string()),
            ::brush_parser::word::ParameterExpr::UseDefaultValues {
                parameter: ::brush_parser::word::Parameter::Named(name),
                indirect: false,
                default_value,
                ..
            } => {
                let held = self.variable(name).unwrap_or_default();
                if held.is_empty() {
                    Some(default_value.clone().unwrap_or_default())
                } else {
                    Some(held.to_string())
                }
            }
            _ => None,
        }
    }
}
