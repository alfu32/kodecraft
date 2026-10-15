use crate::{
    frontend::{c, kotlin_completion_context},
    ids::{FileId, SemanticIds},
    model::SourceRange,
};
use kode_core::{ByteOffset, Result};
use kode_syntax::registry::canonical_language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionContext {
    pub file_id: FileId,
    pub language_id: String,
    pub offset: ByteOffset,
    pub prefix: String,
    pub member_access: bool,
    pub receiver_range: Option<SourceRange>,
    pub import_context: bool,
    pub replacement_range: SourceRange,
    pub qualified_prefix: Option<String>,
    pub content_hash: String,
}
impl CompletionContext {
    pub fn at(file_id: FileId, language: &str, text: &str, offset: ByteOffset) -> Result<Self> {
        SourceRange::caret(offset).validate(text)?;
        let language_id = canonical_language(language);
        let dollar = language_id != "kotlin" && language_id != "c";
        let start = text[..offset.0]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || (dollar && *c == '$'))
            .last()
            .map_or(offset.0, |(index, _)| index);
        let prefix = text[start..offset.0].to_owned();
        let context = if language_id == "c" {
            c::completion_context(text, ByteOffset(start))?
        } else {
            kotlin_completion_context(text, ByteOffset(start))?
        };
        let line_start = text[..offset.0].rfind('\n').map_or(0, |at| at + 1);
        let line = &text[line_start..offset.0];
        let trimmed = line.trim_start();
        let import = trimmed
            .strip_prefix("import")
            .filter(|s| s.starts_with(char::is_whitespace));
        let import_context = import.is_some();
        let mut replacement_range = SourceRange::new(ByteOffset(start), offset)?;
        let qualified_prefix = import.and_then(|suffix| {
            let path = suffix.trim_start();
            // Quoted module paths and aliases retain normal identifier behavior.
            if path
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$'))
            {
                replacement_range.start = ByteOffset(offset.0 - path.len());
                Some(path.to_owned())
            } else {
                None
            }
        });
        Ok(Self {
            file_id,
            language_id,
            offset,
            prefix,
            member_access: !import_context && context.member_access,
            receiver_range: if import_context {
                None
            } else {
                context.receiver_range
            },
            import_context,
            replacement_range,
            qualified_prefix,
            content_hash: SemanticIds::content_hash(text),
        })
    }
}
