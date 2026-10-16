//! Help is generated from the same scoped chord catalog used by reducers.
pub fn new() -> kode_core::Result<crate::widgets::text_panel::TextPanel> {
    crate::widgets::text_panel::TextPanel::new(crate::input::shortcuts::help_lines())
}
