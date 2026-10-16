//! Shared command chords consumed by reducers and the generated Help panel.
//! Text entry is not a shortcut: fields and editor insertion retain original bytes.
use super::terminal::{Key, KeyInput, Modifiers};
const CTRL: u8 = 1;
const ALT: u8 = 2;
const SHIFT: u8 = 4;
const META: u8 = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Editor,
    Search,
    Files,
    Project,
    Git,
    GitDiff,
    Database,
    Sql,
    SqlResults,
    Rest,
    Environment,
    Request,
    Runner,
    Image,
    Hex,
    Lsp,
    Index,
    ProjectSearch,
}
impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Self::Global => "Workspace",
            Self::Editor => "Shared editor",
            Self::Search => "Find/replace bar",
            Self::Files => "Files tree",
            Self::Project => "Project roots and exclusions",
            Self::Git => "Git panel",
            Self::GitDiff => "Git diff",
            Self::Database => "Database explorer",
            Self::Sql => "SQL console",
            Self::SqlResults => "SQL results",
            Self::Rest => "REST collection",
            Self::Environment => "REST environments",
            Self::Request => "REST request editor",
            Self::Runner => "REST run history",
            Self::Image => "Image viewer",
            Self::Hex => "Hex viewer",
            Self::Lsp => "Settings: LSP",
            Self::Index => "Settings: native index",
            Self::ProjectSearch => "Project search dialog",
        }
    }
}
pub struct Binding {
    pub scope: Scope,
    pub keys: &'static [Key],
    required: u8,
    allowed: u8,
    fold_case: bool,
    pub description: &'static str,
}
macro_rules! chord {($scope:ident,$required:expr,$allowed:expr,$fold:expr,[$($key:expr),+],$description:literal)=>{Binding{scope:Scope::$scope,keys:&[$($key),+],required:$required,allowed:$allowed,fold_case:$fold,description:$description}};}
macro_rules! nav {
    ($scope:ident) => {
        chord!(
            $scope,
            0,
            15,
            false,
            [
                Key::Up,
                Key::Down,
                Key::PageUp,
                Key::PageDown,
                Key::Home,
                Key::End
            ],
            "Move or scroll selection; Home/End and Page keys are available"
        )
    };
}
pub static BINDINGS: &[Binding] = &[
    chord!(
        Editor,
        CTRL | SHIFT,
        CTRL | SHIFT,
        false,
        [Key::Up, Key::Down],
        "Add a cursor on the previous / next visual row"
    ),
    chord!(
        Editor,
        0,
        0,
        false,
        [Key::Escape],
        "Keep only the primary cursor"
    ),
    chord!(
        Search,
        ALT,
        ALT,
        false,
        [Key::Enter],
        "Select every occurrence with multiple cursors and return to editing"
    ),
    chord!(
        Global,
        ALT,
        15,
        true,
        [Key::Character('f')],
        "Open project search"
    ),
    chord!(
        Global,
        CTRL | ALT,
        CTRL | ALT | SHIFT,
        true,
        [Key::Character('f')],
        "Open project search (alias for Alt+F)"
    ),
    chord!(
        SqlResults,
        0,
        SHIFT,
        false,
        [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::Tab
        ],
        "Scroll focused results; Tab cycles result sets/messages and Shift reverses"
    ),
    chord!(
        Global,
        0,
        15,
        false,
        [Key::Enter],
        "Dismiss the welcome screen when shown"
    ),
    chord!(
        Global,
        CTRL,
        CTRL | SHIFT,
        true,
        [Key::Character('q')],
        "Save workspace state and quit"
    ),
    chord!(
        Global,
        CTRL,
        CTRL | SHIFT,
        true,
        [Key::Character('t')],
        "Suspend the terminal and open the workspace shell"
    ),
    chord!(
        Global,
        CTRL | SHIFT,
        CTRL | SHIFT | META,
        true,
        [Key::Character('f')],
        "Open project search (alias for Alt+F; terminal hosts may intercept this chord)"
    ),
    chord!(
        Global,
        ALT,
        15,
        false,
        [
            Key::Character('1'),
            Key::Character('2'),
            Key::Character('3'),
            Key::Character('4'),
            Key::Character('5'),
            Key::Character('6'),
            Key::Character('7'),
            Key::Character('8')
        ],
        "Files / Project / Git / Database / REST / About / Settings / Help"
    ),
    chord!(
        Global,
        0,
        15,
        false,
        [Key::Function(6)],
        "Switch focus between panel and editor/viewer"
    ),
    chord!(
        Global,
        0,
        15,
        false,
        [Key::Function(7)],
        "Open run history while REST is active"
    ),
    chord!(
        Editor,
        0,
        15,
        false,
        [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown
        ],
        "Move caret; Shift extends selection; Ctrl+Left/Right moves by word"
    ),
    chord!(
        Editor,
        CTRL,
        15,
        true,
        [
            Key::Character('c'),
            Key::Character('x'),
            Key::Character('v')
        ],
        "Copy / cut / paste (mutation requires an editable host)"
    ),
    chord!(
        Editor,
        ALT,
        15,
        true,
        [
            Key::Character('c'),
            Key::Character('x'),
            Key::Character('v')
        ],
        "Copy / cut / paste aliases"
    ),
    chord!(Editor, CTRL, 15, true, [Key::Character('a')], "Select all"),
    chord!(
        Editor,
        CTRL,
        15,
        true,
        [Key::Character('s')],
        "Save through the current host (file, SQL export or owned draft)"
    ),
    chord!(
        Editor,
        CTRL,
        15,
        true,
        [Key::Character('f')],
        "Open find/replace in this editor"
    ),
    chord!(
        Editor,
        CTRL,
        15,
        false,
        [Key::Character(' ')],
        "Request completion"
    ),
    chord!(
        Editor,
        ALT,
        15,
        true,
        [Key::Character('u'), Key::Character('r')],
        "Undo / redo"
    ),
    chord!(
        Editor,
        0,
        15,
        false,
        [Key::Enter, Key::Tab],
        "Insert newline / indentation when editable"
    ),
    chord!(
        Editor,
        0,
        15,
        false,
        [Key::Backspace, Key::Delete],
        "Delete backward / forward when editable"
    ),
    chord!(
        Search,
        0,
        SHIFT | ALT | META,
        false,
        [Key::Enter],
        "Find next"
    ),
    chord!(
        Search,
        CTRL,
        15,
        false,
        [Key::Enter],
        "Find all occurrences"
    ),
    chord!(
        Search,
        CTRL,
        CTRL | ALT | META,
        true,
        [Key::Character('r')],
        "Replace one occurrence"
    ),
    chord!(
        Search,
        CTRL | SHIFT,
        15,
        true,
        [Key::Character('r')],
        "Replace all occurrences"
    ),
    chord!(
        Search,
        0,
        15,
        false,
        [Key::Tab, Key::Escape],
        "Switch field / close search"
    ),
    nav!(Files),
    chord!(
        Files,
        0,
        SHIFT,
        true,
        [Key::Character('p')],
        "Toggle project-relative / full paths in Recent (also click [paths:rel/full])"
    ),
    chord!(
        Files,
        0,
        SHIFT,
        false,
        [Key::Left, Key::Right, Key::Enter],
        "Collapse / open file or toggle folder"
    ),
    chord!(
        Files,
        0,
        SHIFT,
        false,
        [Key::Function(2), Key::Delete],
        "Rename / delete selected entry"
    ),
    nav!(Project),
    chord!(
        Project,
        0,
        SHIFT,
        true,
        [Key::Character('a'), Key::Character('x')],
        "Add source root / scan exclusion"
    ),
    chord!(
        Project,
        0,
        SHIFT,
        true,
        [
            Key::Character('r'),
            Key::Character('b'),
            Key::Character('c')
        ],
        "Rescan / rebuild native index / clear native index"
    ),
    chord!(
        Project,
        0,
        SHIFT,
        true,
        [Key::Character('d'), Key::Delete, Key::Backspace],
        "Remove selected root or exclusion"
    ),
    nav!(Git),
    chord!(
        Git,
        0,
        15,
        false,
        [Key::Tab],
        "Cycle status/message/history; use Ctrl+Tab from the message editor"
    ),
    chord!(
        Git,
        CTRL,
        15,
        false,
        [Key::Tab],
        "Cycle sections from the message editor; Shift reverses"
    ),
    chord!(
        Git,
        CTRL,
        15,
        false,
        [Key::Enter],
        "Commit the shared message draft"
    ),
    chord!(
        Git,
        0,
        15,
        true,
        [Key::Character('r'), Key::Character('b')],
        "Refresh / choose a branch (outside message editor)"
    ),
    chord!(
        Git,
        0,
        15,
        false,
        [Key::Character('t'), Key::Character('T')],
        "Create tag / explicitly create or move a tag"
    ),
    chord!(
        Git,
        0,
        15,
        true,
        [Key::Character('i')],
        "Initialize when no repository exists"
    ),
    chord!(
        Git,
        0,
        15,
        false,
        [
            Key::Character(' '),
            Key::Character('s'),
            Key::Character('u')
        ],
        "Toggle stage / stage / unstage selected status"
    ),
    chord!(
        Git,
        0,
        15,
        false,
        [Key::Character('a'), Key::Character('A')],
        "Stage all / unstage all"
    ),
    chord!(
        Git,
        0,
        15,
        false,
        [Key::Enter],
        "Open status diff or expand selected history"
    ),
    chord!(
        Git,
        0,
        15,
        false,
        [Key::Character('m')],
        "Copy selected commit message into the draft"
    ),
    chord!(
        GitDiff,
        0,
        SHIFT,
        true,
        [Key::Character('c'), Key::Character('v')],
        "Toggle context / unified or side-by-side view"
    ),
    chord!(
        GitDiff,
        0,
        SHIFT,
        true,
        [Key::Character('r'), Key::Enter],
        "Restore selected unstaged text chunk; historical/staged views are read-only"
    ),
    nav!(Database),
    chord!(
        Database,
        0,
        SHIFT,
        false,
        [Key::Enter, Key::Right, Key::Left],
        "Open or expand / collapse metadata or saved SQL session"
    ),
    chord!(
        Database,
        0,
        SHIFT,
        false,
        [Key::Character('n'), Key::Function(2), Key::Delete],
        "New / edit / delete datasource or selected SQL session"
    ),
    chord!(
        Database,
        0,
        SHIFT,
        false,
        [
            Key::Character('c'),
            Key::Character('s'),
            Key::Character('r'),
            Key::Character('R')
        ],
        "Connect or disconnect / SQL console / refresh / restart external JDBC service"
    ),
    chord!(
        Sql,
        CTRL,
        CTRL,
        false,
        [Key::Enter],
        "Execute selected SQL or current statement"
    ),
    chord!(
        Sql,
        CTRL | SHIFT,
        CTRL | SHIFT,
        false,
        [Key::Enter],
        "Execute script"
    ),
    chord!(
        Sql,
        CTRL | ALT,
        CTRL | ALT | SHIFT,
        true,
        [
            Key::Character('a'),
            Key::Character('c'),
            Key::Character('r')
        ],
        "Toggle shared autocommit / commit / rollback"
    ),
    chord!(
        Sql,
        0,
        15,
        false,
        [Key::Escape],
        "Request cancellation of the active SQL execution"
    ),
    chord!(
        SqlResults,
        ALT,
        ALT | SHIFT,
        false,
        [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End
        ],
        "Scroll result cells while the editor retains focus"
    ),
    chord!(
        SqlResults,
        ALT,
        ALT | SHIFT,
        false,
        [Key::Tab],
        "Cycle result sets and messages"
    ),
    chord!(
        Database,
        0,
        0,
        false,
        [Key::Function(5)],
        "Offer or show the optional Java runtime download when Java is unavailable"
    ),
    chord!(
        Database,
        0,
        0,
        false,
        [Key::Function(4)],
        "Open wrapped database diagnostic details; Ctrl+C copies; Esc closes"
    ),
    chord!(
        Sql,
        0,
        0,
        false,
        [Key::Function(4)],
        "Open SQL diagnostic details; Ctrl+C copies; Esc closes"
    ),
    nav!(Rest),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [Key::Tab],
        "Switch collection / environments focus"
    ),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [Key::Enter, Key::Left, Key::Right],
        "Open request or toggle/collapse folder"
    ),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [Key::Character('/')],
        "Filter the collection"
    ),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [
            Key::Character('n'),
            Key::Character('f'),
            Key::Character('r')
        ],
        "Add request / add folder / run or stop selection"
    ),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [
            Key::Character('i'),
            Key::Character('e'),
            Key::Character('m'),
            Key::Character('!')
        ],
        "Import Postman / export / actions (including OpenAPI import) / notices"
    ),
    chord!(
        Rest,
        0,
        SHIFT,
        false,
        [Key::Function(2), Key::Delete],
        "Rename / delete selected node"
    ),
    nav!(Environment),
    chord!(
        Environment,
        0,
        SHIFT,
        false,
        [Key::Tab],
        "Switch back to collection focus"
    ),
    chord!(
        Environment,
        0,
        SHIFT,
        false,
        [
            Key::Character('n'),
            Key::Character('+'),
            Key::Character('c'),
            Key::Character('m')
        ],
        "New environment / new alias / copy / environment actions"
    ),
    chord!(
        Environment,
        0,
        SHIFT,
        false,
        [
            Key::Enter,
            Key::Character(' '),
            Key::Function(2),
            Key::Delete
        ],
        "Edit values / activate / rename / remove environment"
    ),
    chord!(
        Request,
        CTRL,
        CTRL | SHIFT,
        false,
        [Key::Enter],
        "Send or stop the current request"
    ),
    chord!(
        Request,
        CTRL,
        15,
        false,
        [Key::Tab],
        "Cycle request tabs; Shift reverses"
    ),
    nav!(Runner),
    chord!(
        Runner,
        0,
        SHIFT,
        false,
        [Key::Enter, Key::Escape],
        "Inspect selected outcome / close run history"
    ),
    chord!(
        Runner,
        0,
        SHIFT,
        true,
        [Key::Character('r'), Key::Character('s')],
        "Run again / stop the current run"
    ),
    chord!(
        Image,
        0,
        SHIFT,
        false,
        [Key::Left, Key::Right, Key::Enter],
        "Decrease/increase width / cycle rendering mode"
    ),
    chord!(
        Image,
        0,
        SHIFT,
        true,
        [Key::Character('m')],
        "Toggle advanced image controls"
    ),
    nav!(Hex),
    chord!(
        Hex,
        0,
        0,
        false,
        [Key::Escape],
        "Stop / close byte find without changing selected bytes"
    ),
    chord!(
        Hex,
        CTRL,
        CTRL,
        true,
        [Key::Character('f')],
        "Find bytes: UTF-8 text / hex (Tab switches mode); Enter next, Shift+Enter previous; Escape stops/closes"
    ),
    chord!(
        Hex,
        0,
        SHIFT,
        false,
        [Key::Function(3)],
        "Next byte match; Shift+F3 previous (wraps at EOF)"
    ),
    chord!(
        Hex,
        0,
        0,
        false,
        [Key::Function(8)],
        "Cycle 8 / 16 / 32 bytes per row; header buttons select directly"
    ),
    chord!(
        Hex,
        0,
        15,
        false,
        [Key::Left, Key::Right],
        "Move byte cursor; Shift extends selection"
    ),
    chord!(
        Hex,
        ALT,
        ALT | SHIFT,
        true,
        [
            Key::Character('c'),
            Key::Character('x'),
            Key::Character('v')
        ],
        "Copy / cut / paste local bytes; binary saving is unavailable"
    ),
    chord!(
        Hex,
        CTRL,
        CTRL | SHIFT,
        true,
        [Key::Character('a')],
        "Select all bytes"
    ),
    chord!(
        Hex,
        0,
        15,
        false,
        [Key::Delete, Key::Backspace],
        "Delete selected bytes in memory"
    ),
    nav!(Lsp),
    chord!(
        Lsp,
        0,
        SHIFT,
        true,
        [Key::Character('f'), Key::Function(5)],
        "Refresh catalog and discovery"
    ),
    chord!(
        Lsp,
        0,
        SHIFT,
        true,
        [Key::Character('s'), Key::Character('r')],
        "Start/stop / restart the selected actual server"
    ),
    chord!(
        Lsp,
        0,
        SHIFT,
        false,
        [Key::Enter, Key::Character(' ')],
        "Install or update selected server"
    ),
    chord!(
        Lsp,
        0,
        SHIFT,
        true,
        [
            Key::Character('y'),
            Key::Delete,
            Key::Backspace,
            Key::Escape
        ],
        "Uninstall aliases / request cancellation with Escape"
    ),
    chord!(
        Index,
        0,
        SHIFT,
        true,
        [
            Key::Character('r'),
            Key::Character('b'),
            Key::Character('c')
        ],
        "Rescan / rebuild / clear only the native cache"
    ),
    nav!(ProjectSearch),
    chord!(
        ProjectSearch,
        0,
        15,
        false,
        [Key::Escape, Key::Function(1)],
        "Close dialog / toggle contextual help"
    ),
    chord!(
        ProjectSearch,
        0,
        15,
        false,
        [Key::Tab],
        "Cycle fields; Ctrl+Tab leaves the embedded editor"
    ),
    chord!(
        ProjectSearch,
        CTRL,
        CTRL | SHIFT,
        true,
        [Key::Character('s')],
        "Save the embedded editor"
    ),
    chord!(
        ProjectSearch,
        0,
        SHIFT | ALT | META,
        false,
        [Key::Enter],
        "Next match from fields or open selected match from the results list"
    ),
    chord!(
        ProjectSearch,
        CTRL,
        15,
        false,
        [Key::Enter],
        "Run project search from the query/replacement/filter fields"
    ),
];
fn bits(modifiers: Modifiers) -> u8 {
    (u8::from(modifiers.ctrl) * CTRL)
        | (u8::from(modifiers.alt) * ALT)
        | (u8::from(modifiers.shift) * SHIFT)
        | (u8::from(modifiers.meta) * META)
}
impl Binding {
    pub fn matches(&self, input: KeyInput) -> bool {
        let flags = bits(input.modifiers);
        if flags & self.required != self.required || flags & !self.allowed != 0 {
            return false;
        }
        let key = if self.fold_case {
            match input.key {
                Key::Character(ch) => Key::Character(ch.to_ascii_lowercase()),
                key => key,
            }
        } else {
            input.key
        };
        self.keys.contains(&key)
    }
    pub fn label(&self) -> String {
        let mut prefix = String::new();
        for (flag, name) in [
            (CTRL, "Ctrl+"),
            (ALT, "Alt+"),
            (SHIFT, "Shift+"),
            (META, "Meta+"),
        ] {
            if self.required & flag != 0 {
                prefix.push_str(name);
            }
        }
        self.keys
            .iter()
            .map(|key| {
                format!(
                    "{prefix}{}",
                    match key {
                        Key::Character(' ') => "Space".into(),
                        Key::Character(ch) if self.required & (CTRL | ALT) != 0 =>
                            ch.to_ascii_uppercase().to_string(),
                        Key::Character(ch) => ch.to_string(),
                        Key::Function(number) => format!("F{number}"),
                        key => format!("{key:?}"),
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}
pub fn command_key(scope: Scope, input: KeyInput) -> Option<KeyInput> {
    BINDINGS
        .iter()
        .find(|binding| binding.scope == scope && binding.matches(input))
        .map(|binding| {
            let key = if binding.fold_case {
                match input.key {
                    Key::Character(ch) => Key::Character(ch.to_ascii_lowercase()),
                    key => key,
                }
            } else {
                input.key
            };
            KeyInput { key, ..input }
        })
}
pub fn help_lines() -> Vec<String> {
    let mut lines=vec!["Kode shortcuts".into(),"Only the focused owner receives scoped actions. Modal dialogs and search fields take priority.".into(),"Up/Down/Page/Home/End or wheel scroll Help. F6 returns to the editor.".into(),String::new()];
    for scope in [
        Scope::Global,
        Scope::Editor,
        Scope::Search,
        Scope::Files,
        Scope::Project,
        Scope::Git,
        Scope::GitDiff,
        Scope::Database,
        Scope::Sql,
        Scope::SqlResults,
        Scope::Rest,
        Scope::Environment,
        Scope::Request,
        Scope::Runner,
        Scope::Image,
        Scope::Hex,
        Scope::Lsp,
        Scope::Index,
        Scope::ProjectSearch,
    ] {
        lines.push(format!("{}:", scope.title()));
        for binding in BINDINGS.iter().filter(|binding| binding.scope == scope) {
            lines.push(format!("{} — {}", binding.label(), binding.description));
        }
        lines.push(String::new());
    }
    lines.extend([
        "Pointer actions: Ctrl+click follows definitions or opens usages at a definition; Alt+wheel accelerates editor scrolling.".into(),
        "Multiple cursors: Alt+left-click adds or removes a cursor; Alt+drag adds a selection. Escape keeps the primary. Copy/cut joins selected text in source order with newlines; paste repeats the full text at each selection.".into(),
        "Word selection: double-click selects a Unicode segment; Alt+double-click adds it without losing existing selections. Click pairs use 350 ms; a third click starts a new pair. Whitespace/punctuation are segments, while blank line ends stay carets.".into(),
        "Conflict: Ctrl+Enter means Find all in search, SQL execution in SQL, Send in REST, and Commit in Git. The active owner decides.".into(),
        "Conflict: plain Tab inserts indentation in shared editors; use Ctrl+Tab for Git/REST sections or project-search fields. Settings/About use plain Tab.".into(),
        "Read-only previews allow selection, copy, search and navigation; mutation and ordinary Save stay guarded. Raw REST preview retains its owner's Send action.".into(),
        "Terminal limits: classic encodings may merge Ctrl+Enter with Enter. Use SQL Execute/REST Send buttons. Browser tabs reserve Ctrl+T/Ctrl+W; Chrome --app mode is the existing workaround.".into(),
        "Connection dialog: Alt+T tests, Alt+D installs a driver, Ctrl+S saves. F5 offers Java when unavailable. URL Down opens Standard/Recents.".into(),
        "About: Alt+6, Tab switches credits/languages, and each list scrolls independently. Click a language to select it.".into(),
    ]);
    lines
}
