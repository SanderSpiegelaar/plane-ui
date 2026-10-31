//! AeroSpace 0.21.3 commands and the boundary between editable and read-only scripts.
//!
//! Syntax follows the parsers at
//! <https://github.com/nikitabobko/AeroSpace/tree/v0.21.3-Beta/Sources/Common/cmdArgs/impl>.

use std::collections::BTreeMap;
use std::iter::Peekable;
use std::str::Chars;

use toml_edit::{Item, Value};

use crate::keys::ModeName;

/// AeroSpace version whose command syntax the catalogue models.
pub const CATALOGUE_VERSION: &str = "0.21.3";

/// A command or script that cannot be represented safely by the form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct Unsupported {
    /// Human-readable reason to keep the original value read-only.
    pub reason: String,
}

fn unsupported(reason: impl Into<String>) -> Unsupported {
    Unsupported {
        reason: reason.into(),
    }
}

macro_rules! text_enum {
    ($(#[$meta:meta])* pub enum $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($(#[$variant_meta])* $variant),+ }
        impl $name {
            fn parse(text: &str) -> Result<Self, Unsupported> {
                match text {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(unsupported(format!("invalid {} `{text}`", stringify!($name)))),
                }
            }
            const fn text(self) -> &'static str {
                match self { $(Self::$variant => $text,)+ }
            }
        }
    };
}

text_enum! {
    /// Cardinal direction in the window tree or monitor arrangement.
    pub enum Direction {
        /// Left.
        Left => "left", /// Down.
        Down => "down", /// Up.
        Up => "up", /// Right.
        Right => "right",
    }
}
text_enum! {
    /// Relative position in a workspace or monitor sequence.
    pub enum NextPrev {
        /// Next item.
        Next => "next", /// Previous item.
        Prev => "prev",
    }
}
text_enum! {
    /// Dimension affected by resizing.
    pub enum ResizeDimension {
        /// Infer from the parent orientation.
        Smart => "smart", /// Opposite of the inferred dimension.
        SmartOpposite => "smart-opposite", /// Width.
        Width => "width", /// Height.
        Height => "height",
    }
}
text_enum! {
    /// Layout descriptions accepted by the layout command.
    pub enum LayoutOption {
        /// Tiled layout.
        Tiles => "tiles", /// Accordion layout.
        Accordion => "accordion", /// Horizontal orientation.
        Horizontal => "horizontal", /// Vertical orientation.
        Vertical => "vertical", /// Horizontal tiles.
        HorizontalTiles => "h_tiles", /// Vertical tiles.
        VerticalTiles => "v_tiles", /// Horizontal accordion.
        HorizontalAccordion => "h_accordion", /// Vertical accordion.
        VerticalAccordion => "v_accordion", /// Return to tiling.
        Tiling => "tiling", /// Float the window.
        Floating => "floating",
    }
}
text_enum! {
    /// Boundary of a directional operation.
    pub enum Boundary {
        /// The current workspace.
        Workspace => "workspace", /// The outer frame of all monitors.
        AllMonitorsOuterFrame => "all-monitors-outer-frame",
    }
}
text_enum! {
    /// Behavior when focus reaches a boundary.
    pub enum FocusBoundaryAction {
        /// Stop at the boundary.
        Stop => "stop", /// Return failure.
        Fail => "fail", /// Wrap within the workspace.
        WrapWorkspace => "wrap-around-the-workspace", /// Wrap across all monitors.
        WrapAllMonitors => "wrap-around-all-monitors",
    }
}
text_enum! {
    /// Behavior when movement reaches a boundary.
    pub enum MoveBoundaryAction {
        /// Stop at the boundary.
        Stop => "stop", /// Return failure.
        Fail => "fail", /// Create a container when necessary.
        CreateImplicitContainer => "create-implicit-container",
    }
}

const RESERVED_WORKSPACE_NAMES: &[&str] = &[
    "focused",
    "non-focused",
    "visible",
    "invisible",
    "non-visible",
    "active",
    "non-active",
    "inactive",
    "back-and-forth",
    "back_and_forth",
    "previous",
    "prev",
    "next",
    "monitor",
    "workspace",
    "monitors",
    "workspaces",
    "all",
    "none",
    "mouse",
    "target",
];

fn has_forbidden_characters(name: &str) -> bool {
    let mixed_quotes = name.contains('\'') && name.contains('"');
    mixed_quotes
        || name.starts_with(['_', '-'])
        || name.contains(|character: char| character == ',' || character.is_whitespace())
}

/// A workspace name validated against AeroSpace's reserved names and characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceName(String);

impl WorkspaceName {
    /// Validates a workspace name.
    ///
    /// # Errors
    /// Returns an error for names AeroSpace reserves or forbids.
    pub fn new(name: impl Into<String>) -> Result<Self, Unsupported> {
        let name = name.into();
        if name.is_empty()
            || RESERVED_WORKSPACE_NAMES.contains(&name.as_str())
            || has_forbidden_characters(&name)
        {
            return Err(unsupported(format!("invalid workspace name `{name}`")));
        }
        Ok(Self(name))
    }

    /// The original name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Workspace selected by name or sequence position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceTarget {
    /// A concrete workspace.
    Name(WorkspaceName),
    /// A relative workspace, optionally wrapping at the end of the sequence.
    Relative {
        /// Sequence direction.
        direction: NextPrev,
        /// Wrap at the sequence boundary.
        wrap_around: bool,
        /// Explicit stdin selection, if present.
        stdin: Option<bool>,
    },
}

/// Monitor selected by direction, order, or ordered fallback patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorTarget {
    /// A spatial neighbor.
    Direction(Direction),
    /// A sequence neighbor.
    Relative(NextPrev),
    /// First matching pattern, including `main`, `secondary`, or a 1-based index.
    Patterns(Vec<String>),
}

/// Focus destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    /// Cardinal neighbor.
    Direction(Direction),
    /// Depth-first neighbor.
    Dfs(NextPrev),
    /// Explicit window ID.
    Window(u32),
    /// Zero-based depth-first index.
    DfsIndex(u32),
}

/// Absolute or relative resize amount. Zero retains its original operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeAmount {
    /// Set an absolute size.
    Set(u64),
    /// Add pixels.
    Add(u64),
    /// Subtract pixels.
    Subtract(u64),
}

/// Fullscreen transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenState {
    /// Toggle the current state.
    Toggle,
    /// Enter fullscreen.
    On,
    /// Leave fullscreen.
    Off,
}

/// Focus boundary selection, keeping the wrap alias separate from explicit overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FocusBoundaries {
    /// AeroSpace's default workspace boundary with stop behavior.
    #[default]
    Default,
    /// The `--wrap-around` alias.
    WrapAround,
    /// Explicit boundary behavior.
    Explicit {
        /// Boundary to enforce.
        boundary: Boundary,
        /// Action at the boundary.
        action: FocusBoundaryAction,
    },
}

/// Optional overrides for directional focus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusOptions {
    /// Ignore floating windows.
    pub ignore_floating: bool,
    /// Boundary behavior or wrap alias.
    pub boundaries: FocusBoundaries,
    /// Fail when the current window is fullscreen.
    pub fail_if_fullscreen: bool,
    /// Fail when the current window is in native fullscreen.
    pub fail_if_macos_native_fullscreen: bool,
}

/// Optional overrides for directional movement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveOptions {
    /// Explicit target window.
    pub window_id: Option<u32>,
    /// Explicit boundary override.
    pub boundaries: Option<Boundary>,
    /// Explicit boundary action override.
    pub boundaries_action: Option<MoveBoundaryAction>,
    /// Fail for AeroSpace fullscreen.
    pub fail_if_fullscreen: bool,
    /// Fail for native fullscreen.
    pub fail_if_macos_native_fullscreen: bool,
}

/// Supported commands with typed positional arguments and flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Activate a workspace.
    Workspace {
        /// Destination.
        target: WorkspaceTarget,
        /// Return to the previous workspace when already focused.
        auto_back_and_forth: bool,
        /// Fail when already focused.
        fail_if_noop: bool,
    },
    /// Move a window to a workspace.
    MoveNodeToWorkspace {
        /// Destination.
        target: WorkspaceTarget,
        /// Follow the moved window.
        focus_follows_window: bool,
        /// Fail when already there.
        fail_if_noop: bool,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Switch to the previously focused workspace.
    WorkspaceBackAndForth,
    /// Focus a window.
    Focus {
        /// Destination.
        target: FocusTarget,
        /// Directional behavior.
        options: FocusOptions,
    },
    /// Move a window in the tree.
    Move {
        /// Destination direction.
        direction: Direction,
        /// Movement overrides.
        options: MoveOptions,
    },
    /// Resize a window.
    Resize {
        /// Dimension.
        dimension: ResizeDimension,
        /// Absolute or relative size.
        amount: ResizeAmount,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Cycle through layout descriptions.
    Layout {
        /// Ordered choices.
        options: Vec<LayoutOption>,
        /// Explicit window.
        window_id: Option<u32>,
        /// Explicit workspace, requires root.
        workspace: Option<WorkspaceName>,
        /// Change the root container.
        root: bool,
        /// Fail when unchanged, requires one choice.
        fail_if_noop: bool,
    },
    /// Change fullscreen state.
    Fullscreen {
        /// Transition.
        state: FullscreenState,
        /// Remove outer gaps.
        no_outer_gaps: bool,
        /// Fail when unchanged.
        fail_if_noop: bool,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Join with a neighboring node.
    JoinWith {
        /// Neighbor direction.
        direction: Direction,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Reset the workspace tree.
    FlattenWorkspaceTree {
        /// Explicit workspace.
        workspace: Option<WorkspaceName>,
    },
    /// Equalize window sizes.
    BalanceSizes {
        /// Explicit workspace.
        workspace: Option<WorkspaceName>,
    },
    /// Close a window.
    Close {
        /// Quit its application if this is the last window.
        quit_if_last_window: bool,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Close every other window on the workspace.
    CloseAllWindowsButCurrent {
        /// Quit applications when their last window closes.
        quit_if_last_window: bool,
    },
    /// Focus a monitor.
    FocusMonitor {
        /// Destination.
        target: MonitorTarget,
        /// Wrap relative navigation.
        wrap_around: bool,
    },
    /// Move a window to a monitor.
    MoveNodeToMonitor {
        /// Destination.
        target: MonitorTarget,
        /// Wrap relative navigation.
        wrap_around: bool,
        /// Follow the window.
        focus_follows_window: bool,
        /// Fail when unchanged, patterns only.
        fail_if_noop: bool,
        /// Explicit window.
        window_id: Option<u32>,
    },
    /// Move a workspace to a monitor.
    MoveWorkspaceToMonitor {
        /// Destination.
        target: MonitorTarget,
        /// Wrap relative navigation.
        wrap_around: bool,
        /// Explicit workspace.
        workspace: Option<WorkspaceName>,
    },
    /// Activate a binding mode.
    Mode(ModeName),
    /// Reload the config.
    ReloadConfig {
        /// Suppress AeroSpace's error window.
        no_gui: bool,
        /// Validate without applying.
        dry_run: bool,
        /// Treat warnings as errors.
        warnings_as_errors: bool,
    },
    /// Pass the exact suffix after the command name to bash.
    ExecAndForget(String),
}

/// Argument widget required by the command form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// Cardinal or depth-first direction.
    Direction,
    /// Workspace name or relative target.
    Workspace,
    /// Binding mode name.
    Mode,
    /// Signed or absolute resize amount.
    SignedAmount,
    /// Resize dimension.
    ResizeDimension,
    /// Ordered layout choices.
    LayoutOptions,
    /// Relative monitor or pattern list.
    MonitorTarget,
    /// Verbatim bash suffix.
    ShellText,
    /// Unsigned window ID or index.
    UnsignedInteger,
    /// A fixed set of choices.
    Choice(&'static [&'static str]),
    /// Named flag, optionally followed by an argument.
    Flag(FlagSpec),
}

/// One flag accepted by a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagSpec {
    /// Full spelling including leading dashes.
    pub name: &'static str,
    /// Value kind, or `None` for a boolean flag.
    pub value: Option<&'static ArgKind>,
}

/// One form argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    /// Form label.
    pub name: &'static str,
    /// Widget kind.
    pub kind: ArgKind,
}

/// Metadata for a supported command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// AeroSpace command name.
    pub name: &'static str,
    /// Picker label.
    pub display_name: &'static str,
    /// Short description.
    pub help: &'static str,
    /// Positional arguments and accepted flags.
    pub args: &'static [ArgSpec],
    default_text: &'static str,
}

const fn arg(name: &'static str, kind: ArgKind) -> ArgSpec {
    ArgSpec { name, kind }
}
const fn flag(name: &'static str, value: Option<&'static ArgKind>) -> ArgSpec {
    arg(name, ArgKind::Flag(FlagSpec { name, value }))
}
const WINDOW: ArgSpec = flag("--window-id", Some(&ArgKind::UnsignedInteger));
const WORKSPACE: ArgSpec = flag("--workspace", Some(&ArgKind::Workspace));
const WRAP: ArgSpec = flag("--wrap-around", None);
const NOOP: ArgSpec = flag("--fail-if-noop", None);
const FOLLOW: ArgSpec = flag("--focus-follows-window", None);
const QUIT: ArgSpec = flag("--quit-if-last-window", None);
const FAIL_FULLSCREEN: ArgSpec = flag("--fail-if-fullscreen", None);
const FAIL_NATIVE: ArgSpec = flag("--fail-if-macos-native-fullscreen", None);
const BOUNDARY: ArgSpec = flag(
    "--boundaries",
    Some(&ArgKind::Choice(&["workspace", "all-monitors-outer-frame"])),
);
const STDIN: ArgSpec = flag("--stdin", None);
const NO_STDIN: ArgSpec = flag("--no-stdin", None);

macro_rules! catalogue {
    ($($name:literal, $display:literal, $help:literal, $default:literal, [$($arg:expr),* $(,)?]);+ $(;)?) => {
        /// Supported command metadata, including the complete flag allowlist.
        pub const CATALOGUE: &[CommandSpec] = &[$(CommandSpec {
            name: $name, display_name: $display, help: $help,
            args: &[$($arg),*], default_text: $default,
        }),+];
    };
}

catalogue! {
    "workspace", "Workspace", "Activate a workspace", "workspace 1", [arg("Workspace", ArgKind::Workspace), WRAP, NOOP, flag("--auto-back-and-forth", None), STDIN, NO_STDIN];
    "move-node-to-workspace", "Move to workspace", "Move a window to a workspace", "move-node-to-workspace 1", [arg("Workspace", ArgKind::Workspace), WRAP, NOOP, FOLLOW, WINDOW, STDIN, NO_STDIN];
    "workspace-back-and-forth", "Previous workspace", "Switch to the previous workspace", "workspace-back-and-forth", [];
    "focus", "Focus", "Focus a window", "focus left", [arg("Direction", ArgKind::Direction), flag("--ignore-floating", None), WRAP, BOUNDARY, flag("--boundaries-action", Some(&ArgKind::Choice(&["stop", "fail", "wrap-around-the-workspace", "wrap-around-all-monitors"]))), FAIL_FULLSCREEN, FAIL_NATIVE, WINDOW, flag("--dfs-index", Some(&ArgKind::UnsignedInteger))];
    "move", "Move", "Move a window in the tree", "move left", [arg("Direction", ArgKind::Direction), WINDOW, BOUNDARY, flag("--boundaries-action", Some(&ArgKind::Choice(&["stop", "fail", "create-implicit-container"]))), FAIL_FULLSCREEN, FAIL_NATIVE];
    "resize", "Resize", "Resize a window", "resize smart +50", [arg("Dimension", ArgKind::ResizeDimension), arg("Amount", ArgKind::SignedAmount), WINDOW];
    "layout", "Layout", "Cycle window layouts", "layout tiles", [arg("Layouts", ArgKind::LayoutOptions), WINDOW, WORKSPACE, flag("--root", None), NOOP];
    "fullscreen", "Fullscreen", "Change fullscreen state", "fullscreen", [arg("State", ArgKind::Choice(&["on", "off"])), WINDOW, flag("--no-outer-gaps", None), NOOP];
    "join-with", "Join with", "Group neighboring nodes", "join-with left", [arg("Direction", ArgKind::Direction), WINDOW];
    "flatten-workspace-tree", "Flatten workspace", "Reset the workspace tree", "flatten-workspace-tree", [WORKSPACE];
    "balance-sizes", "Balance sizes", "Equalize window sizes", "balance-sizes", [WORKSPACE];
    "close", "Close", "Close a window", "close", [QUIT, WINDOW];
    "close-all-windows-but-current", "Close other windows", "Close other windows on this workspace", "close-all-windows-but-current", [QUIT];
    "focus-monitor", "Focus monitor", "Focus another monitor", "focus-monitor next", [arg("Monitor", ArgKind::MonitorTarget), WRAP];
    "move-node-to-monitor", "Move to monitor", "Move a window to another monitor", "move-node-to-monitor next", [arg("Monitor", ArgKind::MonitorTarget), WRAP, FOLLOW, NOOP, WINDOW];
    "move-workspace-to-monitor", "Move workspace to monitor", "Move a workspace to another monitor", "move-workspace-to-monitor next", [arg("Monitor", ArgKind::MonitorTarget), WRAP, WORKSPACE];
    "mode", "Mode", "Activate a binding mode", "mode main", [arg("Mode", ArgKind::Mode)];
    "reload-config", "Reload config", "Reload the AeroSpace config", "reload-config", [flag("--no-gui", None), flag("--dry-run", None), flag("--warnings-as-errors", None)];
    "exec-and-forget", "Run shell command", "Run bash without waiting", "exec-and-forget true", [arg("Shell command", ArgKind::ShellText)];
}

impl CommandSpec {
    /// A valid initial command for the picker.
    ///
    /// # Errors
    /// Returns an error if this specification does not describe a known command.
    pub fn default_command(&self) -> Result<Command, Unsupported> {
        Command::parse(self.default_text)
    }
}

/// Finds a command's metadata by its AeroSpace name.
#[must_use]
pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    CATALOGUE.iter().find(|spec| spec.name == name)
}

/// Original TOML representation of a binding script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptShape {
    /// A single TOML string.
    String,
    /// A TOML array, even when it has one element.
    Array,
}

/// The editable model, or an explanation for preserving the original item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedScript {
    /// Every command has a structured representation.
    Editable {
        /// Commands in execution order.
        commands: Vec<Command>,
        /// Original string versus array representation.
        shape: ScriptShape,
    },
    /// The original item must not be rewritten by the command form.
    ReadOnly {
        /// Human-readable explanation.
        reason: String,
    },
}

/// Binding-script parser.
#[derive(Debug)]
pub struct Script;

impl Script {
    /// Parses a string or array without mutating the original TOML item.
    #[must_use]
    pub fn parse(item: &Item) -> ParsedScript {
        let result = if let Some(text) = item.as_str() {
            Command::parse(text).map(|command| (vec![command], ScriptShape::String))
        } else if let Some(array) = item.as_array() {
            array
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| unsupported("script array contains a non-string value"))
                        .and_then(Command::parse)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|commands| (commands, ScriptShape::Array))
        } else {
            Err(unsupported(
                "script must be a string or an array of strings",
            ))
        };
        match result {
            Ok((commands, shape)) => ParsedScript::Editable { commands, shape },
            Err(error) => ParsedScript::ReadOnly {
                reason: error.reason,
            },
        }
    }
}

/// Formats a binding, retaining arrays and promoting strings with multiple commands.
#[must_use]
pub fn format_script(commands: &[Command], shape: ScriptShape) -> Value {
    if let (ScriptShape::String, [command]) = (shape, commands) {
        Value::from(command.format())
    } else {
        Value::Array(commands.iter().map(Command::format).collect())
    }
}

fn words(text: &str) -> Result<Vec<String>, Unsupported> {
    tokenize(text, false).map(|tokens| {
        tokens
            .into_iter()
            .filter_map(|token| match token {
                ScriptToken::Word(word) => Some(word),
                ScriptToken::And => None,
            })
            .collect()
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptToken {
    Word(String),
    And,
}

pub(crate) fn tokenize(text: &str, conditions: bool) -> Result<Vec<ScriptToken>, Unsupported> {
    let mut chars = text.chars().peekable();
    let mut result = Vec::new();
    while let Some(character) = chars.next() {
        if character == '\n' {
            return Err(unsupported("uses a newline outside exec-and-forget"));
        }
        if character.is_whitespace() {
            continue;
        }
        if conditions && character == '&' && chars.peek() == Some(&'&') {
            chars.next();
            result.push(ScriptToken::And);
            continue;
        }
        let word = if matches!(character, '\'' | '"') {
            quoted_word(character, &mut chars, conditions)?
        } else {
            bare_word(character, &mut chars, conditions)?
        };
        result.push(ScriptToken::Word(word));
    }
    Ok(result)
}

fn continues_word(next: char, conditions: bool) -> bool {
    !(next.is_whitespace() || conditions && next == '&')
}

fn quoted_word(
    quote: char,
    chars: &mut Peekable<Chars<'_>>,
    conditions: bool,
) -> Result<String, Unsupported> {
    let mut word = String::new();
    loop {
        match chars.next() {
            Some(next) if next == quote => break,
            Some(next) => word.push(next),
            None => return Err(unsupported("unclosed quote")),
        }
    }
    if chars
        .peek()
        .is_some_and(|next| continues_word(*next, conditions))
    {
        return Err(unsupported("expected whitespace after closing quote"));
    }
    Ok(word)
}

fn bare_word(
    first: char,
    chars: &mut Peekable<Chars<'_>>,
    conditions: bool,
) -> Result<String, Unsupported> {
    let mut word = String::from(first);
    while let Some(next) = chars.next_if(|next| continues_word(*next, conditions)) {
        word.push(next);
    }
    if let Some(operator) = ["&&", "||", ";", "|", "(", ")"]
        .into_iter()
        .find(|operator| word.contains(operator))
    {
        return Err(unsupported(format!("uses the shell operator `{operator}`")));
    }
    if !(conditions && word == "~=") && word.contains(|next| "'\"$~&?*!\\<>`#".contains(next)) {
        return Err(unsupported(format!(
            "unquoted reserved character in `{word}`"
        )));
    }
    Ok(word)
}

/// Commands whose positional argument may follow `--` to escape a leading dash.
const LITERAL_COMMANDS: &[&str] = &[
    "workspace",
    "move-node-to-workspace",
    "mode",
    "focus-monitor",
    "move-node-to-monitor",
    "move-workspace-to-monitor",
];

fn flag_value(
    spec: &CommandSpec,
    token: &str,
    tokens: &mut std::slice::Iter<'_, String>,
) -> Result<Option<String>, Unsupported> {
    let flag = spec
        .args
        .iter()
        .find_map(|arg| match arg.kind {
            ArgKind::Flag(flag) if flag.name == token => Some(flag),
            _ => None,
        })
        .ok_or_else(|| unsupported(format!("unknown flag `{token}` for `{}`", spec.name)))?;
    if flag.value.is_none() {
        return Ok(None);
    }
    tokens
        .next()
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .map(Some)
        .ok_or_else(|| unsupported(format!("missing value for `{token}`")))
}

#[derive(Debug)]
struct Arguments {
    positional: Vec<String>,
    flags: BTreeMap<String, Option<String>>,
    literal: bool,
}

impl Arguments {
    fn parse(spec: &CommandSpec, tokens: &[String]) -> Result<Self, Unsupported> {
        let mut result = Self {
            positional: Vec::new(),
            flags: BTreeMap::new(),
            literal: false,
        };
        let mut tokens = tokens.iter();
        while let Some(token) = tokens.next() {
            if result.literal || !token.starts_with("--") {
                result.positional.push(token.clone());
            } else if token == "--" {
                if !LITERAL_COMMANDS.contains(&spec.name) {
                    return Err(unsupported(format!(
                        "unknown flag `--` for `{}`",
                        spec.name
                    )));
                }
                result.literal = true;
            } else {
                let value = flag_value(spec, token, &mut tokens)?;
                if result.flags.insert(token.clone(), value).is_some() {
                    return Err(unsupported(format!("duplicate flag `{token}`")));
                }
            }
        }
        Ok(result)
    }

    fn has(&self, flag: &str) -> bool {
        self.flags.contains_key(flag)
    }
    fn value(&self, flag: &str) -> Option<&str> {
        self.flags.get(flag).and_then(Option::as_deref)
    }
    fn one(&self) -> Result<&str, Unsupported> {
        match self.positional.as_slice() {
            [value] => Ok(value),
            _ => Err(unsupported("expected exactly one positional argument")),
        }
    }
    fn none(&self) -> Result<(), Unsupported> {
        require(self.positional.is_empty(), "unexpected positional argument")
    }
    fn window(&self) -> Result<Option<u32>, Unsupported> {
        self.value("--window-id").map(unsigned).transpose()
    }
    fn workspace(&self) -> Result<Option<WorkspaceName>, Unsupported> {
        self.value("--workspace")
            .map(WorkspaceName::new)
            .transpose()
    }
    fn workspace_target(&self) -> Result<WorkspaceTarget, Unsupported> {
        let target = self.one()?;
        let relative = !self.literal && matches!(target, "next" | "prev");
        require(
            !(self.has("--stdin") && self.has("--no-stdin")),
            "--stdin and --no-stdin are incompatible",
        )?;
        if relative {
            require(
                !self.has("--fail-if-noop")
                    && !self.has("--auto-back-and-forth")
                    && !self.has("--window-id"),
                "relative workspaces are incompatible with --fail-if-noop, --auto-back-and-forth, or --window-id",
            )?;
            Ok(WorkspaceTarget::Relative {
                direction: NextPrev::parse(target)?,
                wrap_around: self.has("--wrap-around"),
                stdin: if self.has("--stdin") {
                    Some(true)
                } else if self.has("--no-stdin") {
                    Some(false)
                } else {
                    None
                },
            })
        } else {
            require(
                !self.has("--wrap-around") && !self.has("--stdin") && !self.has("--no-stdin"),
                "--wrap-around, --stdin, and --no-stdin require a relative workspace",
            )?;
            Ok(WorkspaceTarget::Name(WorkspaceName::new(target)?))
        }
    }
    fn monitor(&self) -> Result<MonitorTarget, Unsupported> {
        if !self.literal {
            if let [target] = self.positional.as_slice() {
                if let Ok(direction) = Direction::parse(target) {
                    return Ok(MonitorTarget::Direction(direction));
                }
                if let Ok(direction) = NextPrev::parse(target) {
                    return Ok(MonitorTarget::Relative(direction));
                }
            } else if self.positional.first().is_some_and(|target| {
                Direction::parse(target).is_ok() || NextPrev::parse(target).is_ok()
            }) {
                return Err(unsupported(
                    "relative monitor targets accept exactly one argument",
                ));
            }
        }
        require(!self.positional.is_empty(), "missing monitor target")?;
        require(
            !self.has("--wrap-around"),
            "--wrap-around is incompatible with monitor patterns",
        )?;
        for pattern in &self.positional {
            require(
                !pattern.is_empty() && pattern.parse::<i64>().ok().is_none_or(|index| index >= 1),
                "monitor patterns must be nonempty and monitor indices start at 1",
            )?;
            validate_monitor_pattern(pattern)?;
        }
        Ok(MonitorTarget::Patterns(self.positional.clone()))
    }
}

fn validate_monitor_pattern(pattern: &str) -> Result<(), Unsupported> {
    // AeroSpace uses Foundation's ICU regex engine. Without that engine in the portable
    // core, only accept the subset whose syntax we can prove valid rather than guess.
    let mut can_quantify = false;
    let mut chars = pattern.chars().peekable();
    let mut at_start = true;
    while let Some(character) = chars.next() {
        match character {
            '^' if at_start => {}
            '$' if chars.peek().is_none() => {}
            '*' | '+' | '?' if can_quantify => can_quantify = false,
            '\\' => {
                require(
                    chars
                        .next()
                        .is_some_and(|escaped| ".*+?^$[](){}|\\".contains(escaped)),
                    "monitor pattern uses an unsupported regex escape",
                )?;
                can_quantify = true;
            }
            '[' | ']' | '(' | ')' | '{' | '}' | '|' => {
                return Err(unsupported(
                    "monitor pattern uses regex groups, classes, or repetition not modelled by the form",
                ));
            }
            '^' | '$' | '*' | '+' | '?' => {
                return Err(unsupported("invalid monitor pattern anchor or quantifier"));
            }
            _ => can_quantify = true,
        }
        at_start = false;
    }
    Ok(())
}

fn require(valid: bool, reason: &str) -> Result<(), Unsupported> {
    if valid {
        Ok(())
    } else {
        Err(unsupported(reason))
    }
}

fn unsigned(text: &str) -> Result<u32, Unsupported> {
    text.parse()
        .map_err(|_| unsupported(format!("expected an unsigned integer, found `{text}`")))
}

impl Command {
    /// Parses a catalogue command. Shell syntax stays read-only outside `exec-and-forget`.
    ///
    /// # Errors
    /// Returns an explanation for unknown commands, flags, or invalid arguments.
    pub fn parse(text: &str) -> Result<Self, Unsupported> {
        if let Some(suffix) = text.strip_prefix("exec-and-forget")
            && (suffix.is_empty() || suffix.starts_with(char::is_whitespace))
        {
            return Ok(Self::ExecAndForget(suffix.to_owned()));
        }
        let tokens = words(text)?;
        let (name, rest) = tokens
            .split_first()
            .ok_or_else(|| unsupported("empty command"))?;
        let spec =
            command_spec(name).ok_or_else(|| unsupported(format!("unknown command `{name}`")))?;
        let args = Arguments::parse(spec, rest)?;
        Self::from_arguments(spec.name, &args)
    }

    fn from_arguments(name: &str, args: &Arguments) -> Result<Self, Unsupported> {
        match name {
            "workspace" => {
                let target = args.workspace_target()?;
                require(
                    !(args.has("--auto-back-and-forth") && args.has("--fail-if-noop")),
                    "--auto-back-and-forth and --fail-if-noop are incompatible",
                )?;
                Ok(Self::Workspace {
                    target,
                    auto_back_and_forth: args.has("--auto-back-and-forth"),
                    fail_if_noop: args.has("--fail-if-noop"),
                })
            }
            "move-node-to-workspace" => Ok(Self::MoveNodeToWorkspace {
                target: args.workspace_target()?,
                focus_follows_window: args.has("--focus-follows-window"),
                fail_if_noop: args.has("--fail-if-noop"),
                window_id: args.window()?,
            }),
            "workspace-back-and-forth" => {
                args.none()?;
                Ok(Self::WorkspaceBackAndForth)
            }
            "focus" => parse_focus(args),
            "move" => parse_move(args),
            "resize" => parse_resize(args),
            "layout" => parse_layout(args),
            "fullscreen" => parse_fullscreen(args),
            "join-with" => Ok(Self::JoinWith {
                direction: Direction::parse(args.one()?)?,
                window_id: args.window()?,
            }),
            "flatten-workspace-tree" => {
                args.none()?;
                Ok(Self::FlattenWorkspaceTree {
                    workspace: args.workspace()?,
                })
            }
            "balance-sizes" => {
                args.none()?;
                Ok(Self::BalanceSizes {
                    workspace: args.workspace()?,
                })
            }
            "close" => {
                args.none()?;
                Ok(Self::Close {
                    quit_if_last_window: args.has("--quit-if-last-window"),
                    window_id: args.window()?,
                })
            }
            "close-all-windows-but-current" => {
                args.none()?;
                Ok(Self::CloseAllWindowsButCurrent {
                    quit_if_last_window: args.has("--quit-if-last-window"),
                })
            }
            "focus-monitor" => Ok(Self::FocusMonitor {
                target: args.monitor()?,
                wrap_around: args.has("--wrap-around"),
            }),
            "move-node-to-monitor" => {
                let target = args.monitor()?;
                require(
                    !args.has("--fail-if-noop") || matches!(target, MonitorTarget::Patterns(_)),
                    "--fail-if-noop requires monitor patterns",
                )?;
                Ok(Self::MoveNodeToMonitor {
                    target,
                    wrap_around: args.has("--wrap-around"),
                    focus_follows_window: args.has("--focus-follows-window"),
                    fail_if_noop: args.has("--fail-if-noop"),
                    window_id: args.window()?,
                })
            }
            "move-workspace-to-monitor" => Ok(Self::MoveWorkspaceToMonitor {
                target: args.monitor()?,
                wrap_around: args.has("--wrap-around"),
                workspace: args.workspace()?,
            }),
            "mode" => {
                let name = args.one()?;
                if name.contains('\'') && name.contains('"') {
                    return Err(unsupported("mode name cannot be quoted"));
                }
                ModeName::new(name)
                    .map(Self::Mode)
                    .map_err(|error| unsupported(error.to_string()))
            }
            "reload-config" => {
                args.none()?;
                Ok(Self::ReloadConfig {
                    no_gui: args.has("--no-gui"),
                    dry_run: args.has("--dry-run"),
                    warnings_as_errors: args.has("--warnings-as-errors"),
                })
            }
            _ => Err(unsupported(format!("unknown command `{name}`"))),
        }
    }
}

fn parse_focus(args: &Arguments) -> Result<Command, Unsupported> {
    let boundary = args
        .value("--boundaries")
        .map(Boundary::parse)
        .transpose()?;
    let action = args
        .value("--boundaries-action")
        .map(FocusBoundaryAction::parse)
        .transpose()?;
    require(
        !args.has("--wrap-around") || (boundary.is_none() && action.is_none()),
        "--wrap-around is incompatible with --boundaries and --boundaries-action",
    )?;
    require(
        boundary == Some(Boundary::AllMonitorsOuterFrame)
            || action != Some(FocusBoundaryAction::WrapAllMonitors),
        "wrapping all monitors requires --boundaries all-monitors-outer-frame",
    )?;
    let boundaries = if args.has("--wrap-around") {
        FocusBoundaries::WrapAround
    } else if boundary.is_some() || action.is_some() {
        FocusBoundaries::Explicit {
            boundary: boundary.unwrap_or(Boundary::Workspace),
            action: action.unwrap_or(FocusBoundaryAction::Stop),
        }
    } else {
        FocusBoundaries::Default
    };
    let options = FocusOptions {
        ignore_floating: args.has("--ignore-floating"),
        boundaries,
        fail_if_fullscreen: args.has("--fail-if-fullscreen"),
        fail_if_macos_native_fullscreen: args.has("--fail-if-macos-native-fullscreen"),
    };
    let target = if let Some(id) = args.value("--window-id") {
        args.none()?;
        require(
            args.flags.len() == 1,
            "--window-id is incompatible with other focus options",
        )?;
        FocusTarget::Window(unsigned(id)?)
    } else if let Some(index) = args.value("--dfs-index") {
        args.none()?;
        require(
            args.flags.len() == 1,
            "--dfs-index is incompatible with other focus options",
        )?;
        FocusTarget::DfsIndex(unsigned(index)?)
    } else {
        match args.one()? {
            "dfs-next" => FocusTarget::Dfs(NextPrev::Next),
            "dfs-prev" => FocusTarget::Dfs(NextPrev::Prev),
            direction => FocusTarget::Direction(Direction::parse(direction)?),
        }
    };
    if matches!(target, FocusTarget::Dfs(_)) {
        require(
            boundary != Some(Boundary::AllMonitorsOuterFrame),
            "depth-first focus requires workspace boundaries",
        )?;
        require(
            !options.fail_if_fullscreen && !options.fail_if_macos_native_fullscreen,
            "fullscreen failure flags require a cardinal direction",
        )?;
    }
    Ok(Command::Focus { target, options })
}

fn parse_move(args: &Arguments) -> Result<Command, Unsupported> {
    Ok(Command::Move {
        direction: Direction::parse(args.one()?)?,
        options: MoveOptions {
            window_id: args.window()?,
            boundaries: args
                .value("--boundaries")
                .map(Boundary::parse)
                .transpose()?,
            boundaries_action: args
                .value("--boundaries-action")
                .map(MoveBoundaryAction::parse)
                .transpose()?,
            fail_if_fullscreen: args.has("--fail-if-fullscreen"),
            fail_if_macos_native_fullscreen: args.has("--fail-if-macos-native-fullscreen"),
        },
    })
}

fn parse_resize(args: &Arguments) -> Result<Command, Unsupported> {
    let [dimension, amount] = args.positional.as_slice() else {
        return Err(unsupported("resize requires a dimension and an amount"));
    };
    let digits = amount.strip_prefix(['+', '-']).unwrap_or(amount);
    require(
        !digits.starts_with(['+', '-']),
        "resize amount has multiple signs",
    )?;
    let magnitude = digits
        .parse::<u64>()
        .map_err(|_| unsupported(format!("invalid resize amount `{amount}`")))?;
    let amount = if amount.starts_with('+') {
        ResizeAmount::Add(magnitude)
    } else if amount.starts_with('-') {
        ResizeAmount::Subtract(magnitude)
    } else {
        ResizeAmount::Set(magnitude)
    };
    Ok(Command::Resize {
        dimension: ResizeDimension::parse(dimension)?,
        amount,
        window_id: args.window()?,
    })
}

fn parse_fullscreen(args: &Arguments) -> Result<Command, Unsupported> {
    let state = match args.positional.as_slice() {
        [] => FullscreenState::Toggle,
        [state] if state == "on" => FullscreenState::On,
        [state] if state == "off" => FullscreenState::Off,
        _ => {
            return Err(unsupported(
                "fullscreen accepts only on, off, or no argument",
            ));
        }
    };
    require(
        state != FullscreenState::Off || !args.has("--no-outer-gaps"),
        "--no-outer-gaps is incompatible with off",
    )?;
    require(
        state != FullscreenState::Toggle || !args.has("--fail-if-noop"),
        "--fail-if-noop requires on or off",
    )?;
    Ok(Command::Fullscreen {
        state,
        no_outer_gaps: args.has("--no-outer-gaps"),
        fail_if_noop: args.has("--fail-if-noop"),
        window_id: args.window()?,
    })
}

fn parse_layout(args: &Arguments) -> Result<Command, Unsupported> {
    require(
        !args.positional.is_empty(),
        "layout requires at least one layout option",
    )?;
    let options = args
        .positional
        .iter()
        .map(|text| LayoutOption::parse(text))
        .collect::<Result<Vec<_>, _>>()?;
    let root = args.has("--root");
    let workspace = args.workspace()?;
    let window_id = args.window()?;
    require(
        workspace.is_none() || window_id.is_none(),
        "--workspace and --window-id are incompatible",
    )?;
    require(workspace.is_none() || root, "--workspace requires --root")?;
    require(
        !root
            || options
                .iter()
                .all(|option| !matches!(option, LayoutOption::Tiling | LayoutOption::Floating)),
        "--root is incompatible with tiling and floating",
    )?;
    require(
        !args.has("--fail-if-noop") || options.len() == 1,
        "--fail-if-noop requires one layout option",
    )?;
    Ok(Command::Layout {
        options,
        window_id,
        workspace,
        root,
        fail_if_noop: args.has("--fail-if-noop"),
    })
}

pub(crate) fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.:/+=".contains(character))
    {
        text.to_owned()
    } else if !text.contains('\'') {
        format!("'{text}'")
    } else {
        format!("\"{text}\"")
    }
}

#[derive(Debug, Default)]
struct Formatted(Vec<String>);

impl Formatted {
    fn flag(&mut self, name: &str, enabled: bool) {
        if enabled {
            self.0.push(name.to_owned());
        }
    }
    fn value(&mut self, name: &str, value: Option<String>) {
        if let Some(value) = value {
            self.0.extend([name.to_owned(), quote(&value)]);
        }
    }
    fn window(&mut self, window: Option<u32>) {
        self.value("--window-id", window.map(|id| id.to_string()));
    }
    fn workspace(&mut self, workspace: Option<&WorkspaceName>) {
        self.value(
            "--workspace",
            workspace.map(|name| name.as_str().to_owned()),
        );
    }
    fn workspace_target(&mut self, target: &WorkspaceTarget) {
        match target {
            WorkspaceTarget::Name(name) => self.0.push(quote(name.as_str())),
            WorkspaceTarget::Relative {
                direction,
                wrap_around,
                stdin,
            } => {
                self.flag("--wrap-around", *wrap_around);
                if let Some(stdin) = stdin {
                    self.flag(if *stdin { "--stdin" } else { "--no-stdin" }, true);
                }
                self.0.push(direction.text().to_owned());
            }
        }
    }
    fn monitor(&mut self, target: &MonitorTarget) {
        match target {
            MonitorTarget::Direction(direction) => self.0.push(direction.text().to_owned()),
            MonitorTarget::Relative(direction) => self.0.push(direction.text().to_owned()),
            MonitorTarget::Patterns(patterns) => {
                if patterns.first().is_some_and(|pattern| {
                    pattern.starts_with('-')
                        || Direction::parse(pattern).is_ok()
                        || NextPrev::parse(pattern).is_ok()
                }) {
                    self.0.push("--".to_owned());
                }
                self.0.extend(patterns.iter().map(|pattern| quote(pattern)));
            }
        }
    }

    fn focus(&mut self, target: &FocusTarget, options: &FocusOptions) {
        self.flag("--ignore-floating", options.ignore_floating);
        match options.boundaries {
            FocusBoundaries::Default => {}
            FocusBoundaries::WrapAround => self.flag("--wrap-around", true),
            FocusBoundaries::Explicit { boundary, action } => {
                self.value("--boundaries", Some(boundary.text().to_owned()));
                self.value("--boundaries-action", Some(action.text().to_owned()));
            }
        }
        self.flag("--fail-if-fullscreen", options.fail_if_fullscreen);
        self.flag(
            "--fail-if-macos-native-fullscreen",
            options.fail_if_macos_native_fullscreen,
        );
        match target {
            FocusTarget::Direction(direction) => self.0.push(direction.text().to_owned()),
            FocusTarget::Dfs(direction) => self.0.push(format!("dfs-{}", direction.text())),
            FocusTarget::Window(id) => self.window(Some(*id)),
            FocusTarget::DfsIndex(index) => self.value("--dfs-index", Some(index.to_string())),
        }
    }

    fn movement(&mut self, direction: Direction, options: &MoveOptions) {
        self.window(options.window_id);
        self.value(
            "--boundaries",
            options.boundaries.map(|value| value.text().to_owned()),
        );
        self.value(
            "--boundaries-action",
            options
                .boundaries_action
                .map(|value| value.text().to_owned()),
        );
        self.flag("--fail-if-fullscreen", options.fail_if_fullscreen);
        self.flag(
            "--fail-if-macos-native-fullscreen",
            options.fail_if_macos_native_fullscreen,
        );
        self.0.push(direction.text().to_owned());
    }

    fn resize(&mut self, dimension: ResizeDimension, amount: ResizeAmount, window_id: Option<u32>) {
        self.window(window_id);
        self.0.push(dimension.text().to_owned());
        self.0.push(match amount {
            ResizeAmount::Set(value) => value.to_string(),
            ResizeAmount::Add(value) => format!("+{value}"),
            ResizeAmount::Subtract(value) => format!("-{value}"),
        });
    }

    fn layout(
        &mut self,
        options: &[LayoutOption],
        window_id: Option<u32>,
        workspace: Option<&WorkspaceName>,
        root: bool,
        fail_if_noop: bool,
    ) {
        self.window(window_id);
        self.workspace(workspace);
        self.flag("--root", root);
        self.flag("--fail-if-noop", fail_if_noop);
        self.0
            .extend(options.iter().map(|option| option.text().to_owned()));
    }

    fn fullscreen(
        &mut self,
        state: FullscreenState,
        no_outer_gaps: bool,
        fail_if_noop: bool,
        window_id: Option<u32>,
    ) {
        self.window(window_id);
        self.flag("--no-outer-gaps", no_outer_gaps);
        self.flag("--fail-if-noop", fail_if_noop);
        match state {
            FullscreenState::Toggle => {}
            FullscreenState::On => self.0.push("on".to_owned()),
            FullscreenState::Off => self.0.push("off".to_owned()),
        }
    }

    fn move_to_monitor(
        &mut self,
        target: &MonitorTarget,
        wrap_around: bool,
        follow: bool,
        fail_if_noop: bool,
        window_id: Option<u32>,
    ) {
        self.window(window_id);
        self.flag("--wrap-around", wrap_around);
        self.flag("--focus-follows-window", follow);
        self.flag("--fail-if-noop", fail_if_noop);
        self.monitor(target);
    }
}

impl Command {
    /// AeroSpace command name, also the metadata lookup key.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::MoveNodeToWorkspace { .. } => "move-node-to-workspace",
            Self::WorkspaceBackAndForth => "workspace-back-and-forth",
            Self::Focus { .. } => "focus",
            Self::Move { .. } => "move",
            Self::Resize { .. } => "resize",
            Self::Layout { .. } => "layout",
            Self::Fullscreen { .. } => "fullscreen",
            Self::JoinWith { .. } => "join-with",
            Self::FlattenWorkspaceTree { .. } => "flatten-workspace-tree",
            Self::BalanceSizes { .. } => "balance-sizes",
            Self::Close { .. } => "close",
            Self::CloseAllWindowsButCurrent { .. } => "close-all-windows-but-current",
            Self::FocusMonitor { .. } => "focus-monitor",
            Self::MoveNodeToMonitor { .. } => "move-node-to-monitor",
            Self::MoveWorkspaceToMonitor { .. } => "move-workspace-to-monitor",
            Self::Mode(_) => "mode",
            Self::ReloadConfig { .. } => "reload-config",
            Self::ExecAndForget(_) => "exec-and-forget",
        }
    }

    /// Formats canonical AeroSpace syntax. The exec suffix is not escaped or trimmed.
    /// Commands constructed directly must obey the same constraints as [`Self::parse`].
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "One exhaustive match keeps all 19 command formatters together."
    )]
    pub fn format(&self) -> String {
        if let Self::ExecAndForget(suffix) = self {
            return format!("exec-and-forget{suffix}");
        }
        let mut output = Formatted::default();
        match self {
            Self::Workspace {
                target,
                auto_back_and_forth,
                fail_if_noop,
            } => {
                output.flag("--auto-back-and-forth", *auto_back_and_forth);
                output.flag("--fail-if-noop", *fail_if_noop);
                output.workspace_target(target);
            }
            Self::MoveNodeToWorkspace {
                target,
                focus_follows_window,
                fail_if_noop,
                window_id,
            } => {
                output.flag("--focus-follows-window", *focus_follows_window);
                output.flag("--fail-if-noop", *fail_if_noop);
                output.window(*window_id);
                output.workspace_target(target);
            }
            Self::Focus { target, options } => output.focus(target, options),
            Self::Move { direction, options } => output.movement(*direction, options),
            Self::Resize {
                dimension,
                amount,
                window_id,
            } => output.resize(*dimension, *amount, *window_id),
            Self::Layout {
                options,
                window_id,
                workspace,
                root,
                fail_if_noop,
            } => output.layout(
                options,
                *window_id,
                workspace.as_ref(),
                *root,
                *fail_if_noop,
            ),
            Self::Fullscreen {
                state,
                no_outer_gaps,
                fail_if_noop,
                window_id,
            } => output.fullscreen(*state, *no_outer_gaps, *fail_if_noop, *window_id),
            Self::JoinWith {
                direction,
                window_id,
            } => {
                output.window(*window_id);
                output.0.push(direction.text().to_owned());
            }
            Self::FlattenWorkspaceTree { workspace } | Self::BalanceSizes { workspace } => {
                output.workspace(workspace.as_ref());
            }
            Self::Close {
                quit_if_last_window,
                window_id,
            } => {
                output.window(*window_id);
                output.flag("--quit-if-last-window", *quit_if_last_window);
            }
            Self::CloseAllWindowsButCurrent {
                quit_if_last_window,
            } => output.flag("--quit-if-last-window", *quit_if_last_window),
            Self::FocusMonitor {
                target,
                wrap_around,
            } => {
                output.flag("--wrap-around", *wrap_around);
                output.monitor(target);
            }
            Self::MoveNodeToMonitor {
                target,
                wrap_around,
                focus_follows_window,
                fail_if_noop,
                window_id,
            } => output.move_to_monitor(
                target,
                *wrap_around,
                *focus_follows_window,
                *fail_if_noop,
                *window_id,
            ),
            Self::MoveWorkspaceToMonitor {
                target,
                wrap_around,
                workspace,
            } => {
                output.workspace(workspace.as_ref());
                output.flag("--wrap-around", *wrap_around);
                output.monitor(target);
            }
            Self::Mode(mode) => {
                output.flag("--", mode.as_str().starts_with('-'));
                output.0.push(quote(mode.as_str()));
            }
            Self::ReloadConfig {
                no_gui,
                dry_run,
                warnings_as_errors,
            } => {
                output.flag("--no-gui", *no_gui);
                output.flag("--dry-run", *dry_run);
                output.flag("--warnings-as-errors", *warnings_as_errors);
            }
            Self::WorkspaceBackAndForth | Self::ExecAndForget(_) => {}
        }
        std::iter::once(self.name().to_owned())
            .chain(output.0)
            .collect::<Vec<_>>()
            .join(" ")
    }
}
