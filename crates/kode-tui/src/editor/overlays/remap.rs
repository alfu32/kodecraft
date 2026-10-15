//! Move paint with unchanged UTF-8 source, including changes without deltas (undo,
//! another view). The rope is never flattened on the application thread.
use kode_core::{
    ByteOffset, TextRange,
    document::{EditDelta, TextSnapshot},
};

struct Edit {
    start: usize,
    end: usize,
    inserted: usize,
}

fn difference(old: &TextSnapshot, new: &TextSnapshot) -> Edit {
    let mut start = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b {
            break;
        }
        start += a.len_utf8();
    }
    let mut end = old.len_bytes();
    let mut new_end = new.len_bytes();
    let old_slice = old
        .slice(TextRange {
            start: ByteOffset(0),
            end: ByteOffset(end),
        })
        .expect("whole snapshot");
    let new_slice = new
        .slice(TextRange {
            start: ByteOffset(0),
            end: ByteOffset(new_end),
        })
        .expect("whole snapshot");
    for (a, b) in old_slice
        .chars_at(old_slice.len_chars())
        .reversed()
        .zip(new_slice.chars_at(new_slice.len_chars()).reversed())
    {
        if end <= start || new_end <= start || a != b {
            break;
        }
        end -= a.len_utf8();
        new_end -= b.len_utf8();
    }
    Edit {
        start,
        end,
        inserted: new_end - start,
    }
}
fn part(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
}
fn before(text: &TextSnapshot, offset: ByteOffset) -> Option<char> {
    text.slice(TextRange {
        start: ByteOffset(0),
        end: offset,
    })
    .ok()
    .and_then(|slice| slice.chars_at(slice.len_chars()).prev())
}
fn after(text: &TextSnapshot, offset: ByteOffset) -> Option<char> {
    text.slice(TextRange {
        start: offset,
        end: ByteOffset(text.len_bytes()),
    })
    .ok()
    .and_then(|slice| slice.chars().next())
}

/// Deltas in one core transaction share revision stamps and are applied right to
/// left. Unexpected stamps/order fall back to a scalar-safe contiguous diff.
pub fn remap_ranges(
    old: &TextSnapshot,
    new: &TextSnapshot,
    ranges: &[TextRange],
    deltas: Option<&[EditDelta]>,
) -> Vec<Option<TextRange>> {
    if ranges.is_empty() {
        return vec![];
    }
    if old.id != new.id {
        return vec![None; ranges.len()];
    }
    let edits: Vec<_> = match deltas.filter(|deltas| {
        !deltas.is_empty()
            && deltas
                .iter()
                .all(|d| d.previous_revision == old.revision && d.revision == new.revision)
            && deltas
                .windows(2)
                .all(|pair| pair[1].range.end <= pair[0].range.start)
    }) {
        Some(deltas) => deltas
            .iter()
            .map(|d| Edit {
                start: d.range.start.0,
                end: d.range.end.0,
                inserted: d.inserted.len(),
            })
            .collect(),
        None => vec![difference(old, new)],
    };
    ranges
        .iter()
        .map(|original| {
            if original.is_empty() || old.slice(*original).is_err() {
                return None;
            }
            let mut range = *original;
            for edit in &edits {
                if range.end.0 <= edit.start {
                    continue;
                }
                if range.start.0 < edit.end {
                    return None;
                }
                let shift = edit.inserted as i128 - (edit.end - edit.start) as i128;
                range.start = ByteOffset(usize::try_from(range.start.0 as i128 + shift).ok()?);
                range.end = ByteOffset(usize::try_from(range.end.0 as i128 + shift).ok()?);
            }
            new.slice(range).ok()?;
            // Growing foo into foobar/barfoo invalidates cached identifier paint,
            // even though no byte inside the original token was replaced.
            if part(after(old, original.start))
                && part(before(old, original.start)) != part(before(new, range.start))
            {
                return None;
            }
            if part(before(old, original.end))
                && part(after(old, original.end)) != part(after(new, range.end))
            {
                return None;
            }
            Some(range)
        })
        .collect()
}
