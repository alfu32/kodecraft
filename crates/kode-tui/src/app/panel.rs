//! Visual order follows the original TUI. Alt+1..8 follows this same order (B007).
pub const FILES: usize = 0;
pub const PROJECT: usize = 1;
pub const GIT: usize = 2;
pub const DATABASE: usize = 3;
pub const REST: usize = 4;
pub const ABOUT: usize = 5;
pub const SETTINGS: usize = 6;
pub const HELP: usize = 7;
pub const TITLES: [&str; 8] = [
    "Files", "Project", "Git", "Database", "REST", "About", "Settings", "Help",
];
pub const SHORTCUTS: [usize; 8] = [FILES, PROJECT, GIT, DATABASE, REST, ABOUT, SETTINGS, HELP];
