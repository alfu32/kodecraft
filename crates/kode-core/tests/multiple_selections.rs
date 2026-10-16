use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Revision, Selection, TextRange,
    commands::EditKind,
    document::Document,
    editor::{
        edit::{self, TextEdit},
        history::HistoryStateV1,
        selection::CaretState,
    },
};

fn document(text: &str) -> Document {
    Document::new(
        DocumentId(4),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "multiple".into(),
            label: "Multiple".into(),
        },
        None,
        text,
    )
}
fn selection(anchor: usize, head: usize) -> Selection {
    Selection {
        anchor: ByteOffset(anchor),
        head: ByteOffset(head),
    }
}
fn carets(primary: Selection, secondary: &[Selection]) -> CaretState {
    CaretState {
        selection: primary,
        secondary: secondary.to_vec(),
    }
}

#[test]
fn normalization_preserves_primary_direction_merges_overlap_and_keeps_adjacent_ranges() {
    let doc = document("0123456789");
    let input = carets(
        selection(6, 3),
        &[
            selection(1, 4),
            selection(6, 6),
            selection(8, 9),
            selection(9, 10),
            selection(9, 8),
            selection(2, 2),
        ],
    );
    let normalized = input.normalized(&doc.snapshot()).unwrap();
    assert_eq!(
        normalized,
        carets(selection(6, 1), &[selection(8, 9), selection(9, 10)])
    );
    assert_eq!(normalized.normalized(&doc.snapshot()).unwrap(), normalized);
    assert_eq!(input.selection, selection(6, 3));
    let duplicate = carets(
        selection(0, 0),
        &[selection(0, 0), selection(0, 1), selection(1, 1)],
    );
    assert_eq!(
        duplicate.normalized(&doc.snapshot()).unwrap(),
        CaretState::single(selection(0, 1))
    );
}

#[test]
fn unicode_replacements_are_one_revision_and_undo_restores_every_selection() {
    let mut doc = document("éx😀yz\nab");
    let mut state = carets(
        selection(8, 7),
        &[
            selection(0, 2),
            selection(3, 8),
            selection(12, 12),
            selection(12, 12),
        ],
    );
    let before = state.normalized(&doc.snapshot()).unwrap();
    let outcome = edit::insert(&mut doc, &mut state, "Ω", EditKind::Typing)
        .unwrap()
        .unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "ΩxΩz\nabΩ");
    assert_eq!(
        state,
        carets(selection(5, 5), &[selection(2, 2), selection(11, 11)])
    );
    assert_eq!(outcome.deltas.len(), 3);
    assert_eq!(doc.revision(), Revision(1));
    assert_eq!(doc.history_depth(), (1, 0));
    assert!(
        outcome
            .deltas
            .windows(2)
            .all(|pair| pair[0].range.start > pair[1].range.start)
    );
    let after = state.clone();
    doc.undo(&mut state).unwrap();
    assert_eq!(state, before);
    assert_eq!(doc.snapshot().text().as_ref(), "éx😀yz\nab");
    assert!(!doc.is_dirty());
    doc.redo(&mut state).unwrap();
    assert_eq!(state, after);
    assert_eq!(doc.snapshot().text().as_ref(), "ΩxΩz\nabΩ");
}

#[test]
fn source_order_and_primary_choice_do_not_change_text_or_duplicate_paste() {
    let selections = [selection(0, 1), selection(1, 2), selection(3, 3)];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let mut doc = document("abc");
        let mut state = carets(
            selections[order[0]],
            &[selections[order[1]], selections[order[2]]],
        );
        edit::insert(&mut doc, &mut state, "XY", EditKind::Paste).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), "XYXYcXY");
        assert_eq!(state.cursor().0, [2, 4, 7][order[0]]);
        assert_eq!(doc.history_depth(), (1, 0));
        doc.undo(&mut state).unwrap();
        assert_eq!(state.selection, selections[order[0]]);
        assert_eq!(state.secondary.len(), 2);
    }
}

#[test]
fn deletion_merges_colliding_carets_and_keeps_crlf_and_unicode_boundaries() {
    let mut doc = document("abc");
    let mut state = carets(selection(2, 2), &[selection(1, 1)]);
    let before = state.clone();
    edit::delete(&mut doc, &mut state, true).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "c");
    assert_eq!(state, CaretState::single(selection(0, 0)));
    doc.undo(&mut state).unwrap();
    assert_eq!(state, before);
    doc.redo(&mut state).unwrap();
    assert_eq!(state, CaretState::single(selection(0, 0)));

    let mut doc = document("é\r\n😀\r\nx");
    let mut state = carets(selection(4, 4), &[selection(10, 10)]);
    edit::delete(&mut doc, &mut state, true).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "é😀x");
    assert_eq!(state, carets(selection(2, 2), &[selection(6, 6)]));
    doc.undo(&mut state).unwrap();
    edit::delete(&mut doc, &mut state, false).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "é\r\n\r\n");
    assert_eq!(state, carets(selection(4, 4), &[selection(6, 6)]));
}

#[test]
fn newline_uses_each_carets_original_indent_and_break_style() {
    let mut doc = document("  a\r\n\tb");
    let mut state = carets(selection(3, 3), &[selection(7, 7)]);
    let before = state.clone();
    edit::newline(&mut doc, &mut state).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "  a\r\n  \r\n\tb\r\n\t");
    assert_eq!(state, carets(selection(7, 7), &[selection(14, 14)]));
    assert_eq!(doc.history_depth(), (1, 0));
    doc.undo(&mut state).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "  a\r\n\tb");
    assert_eq!(state, before);
}

#[test]
fn history_roundtrip_preserves_sets_and_old_single_selection_json_still_loads() {
    let mut doc = document("a b c");
    let mut state = carets(selection(2, 3), &[selection(0, 1), selection(4, 5)]);
    let original = state.clone();
    edit::insert(&mut doc, &mut state, "界\n", EditKind::Paste).unwrap();
    let inserted = state.clone();
    let exported = doc.export_history(&state);
    let json = serde_json::to_vec(&exported).unwrap();
    let mut restored = document("");
    let mut restored_state = CaretState::default();
    restored
        .import_history(serde_json::from_slice(&json).unwrap(), &mut restored_state)
        .unwrap();
    assert_eq!(restored_state, inserted);
    restored.undo(&mut restored_state).unwrap();
    assert_eq!(restored.snapshot().text().as_ref(), "a b c");
    assert_eq!(restored_state, original);
    let json = serde_json::to_vec(&restored.export_history(&restored_state)).unwrap();
    let mut redo_restored = document("");
    redo_restored
        .import_history(serde_json::from_slice(&json).unwrap(), &mut restored_state)
        .unwrap();
    redo_restored.redo(&mut restored_state).unwrap();
    assert_eq!(redo_restored.snapshot().text().as_ref(), "界\n 界\n 界\n");
    assert_eq!(restored_state, inserted);

    let mut old = serde_json::to_value(exported).unwrap();
    old["current"].as_object_mut().unwrap().remove("secondary");
    for row in old["undo"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("secondary");
    }
    let old: HistoryStateV1 = serde_json::from_value(old).unwrap();
    restored.import_history(old, &mut restored_state).unwrap();
    assert!(restored_state.secondary.is_empty());
    restored.undo(&mut restored_state).unwrap();
    assert_eq!(restored_state.selection, original.selection);
    assert!(restored_state.secondary.is_empty());
}

#[test]
fn invalid_secondary_and_after_state_fail_atomically_and_readonly_guards_all_mutations() {
    let mut doc = document("a😀b");
    let mut invalid = carets(selection(0, 0), &[selection(2, 2)]);
    let before = invalid.clone();
    assert!(edit::insert(&mut doc, &mut invalid, "x", EditKind::Typing).is_err());
    assert_eq!(invalid, before);
    assert_eq!(doc.snapshot().text().as_ref(), "a😀b");
    assert_eq!(doc.revision(), Revision(0));
    let good = carets(selection(0, 0), &[selection(5, 5)]);
    assert!(
        doc.apply_caret_transaction(
            Revision(0),
            vec![TextEdit {
                range: TextRange::caret(ByteOffset(0)),
                inserted: "x".into()
            }],
            EditKind::Typing,
            &good,
            Some(carets(selection(0, 0), &[selection(3, 3)])),
        )
        .is_err()
    );
    assert_eq!(doc.snapshot().text().as_ref(), "a😀b");
    assert_eq!(doc.history_depth(), (0, 0));
    let mut state = good;
    edit::insert(&mut doc, &mut state, "x", EditKind::Typing).unwrap();
    let mut history = doc.export_history(&state);
    history.undo[0].secondary = vec![selection(2, 2)];
    let text = doc.snapshot().text();
    let before = state.clone();
    let revision = doc.revision();
    let depth = doc.history_depth();
    assert!(doc.import_history(history, &mut state).is_err());
    doc.set_read_only(true);
    assert!(matches!(
        edit::insert(&mut doc, &mut state, "x", EditKind::Paste),
        Err(DomainError::ReadOnly)
    ));
    assert!(matches!(
        edit::delete(&mut doc, &mut state, true),
        Err(DomainError::ReadOnly)
    ));
    assert!(matches!(
        edit::newline(&mut doc, &mut state),
        Err(DomainError::ReadOnly)
    ));
    assert!(matches!(doc.undo(&mut state), Err(DomainError::ReadOnly)));
    assert!(matches!(doc.redo(&mut state), Err(DomainError::ReadOnly)));
    assert_eq!(state, before);
    assert_eq!(doc.snapshot().text(), text);
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.history_depth(), depth);
}

#[test]
fn duplicate_empty_cursors_are_noops_and_do_not_discard_redo() {
    let mut doc = document("");
    let mut state = carets(selection(0, 0), &[selection(0, 0)]);
    assert!(edit::delete(&mut doc, &mut state, true).unwrap().is_none());
    assert_eq!(doc.history_depth(), (0, 0));
    edit::insert(&mut doc, &mut state, "x", EditKind::Typing).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "x");
    assert_eq!(state, CaretState::single(selection(1, 1)));
    doc.undo(&mut state).unwrap();
    assert!(
        edit::insert(&mut doc, &mut state, "", EditKind::Typing)
            .unwrap()
            .is_none()
    );
    assert_eq!(doc.history_depth(), (0, 1));
    assert_eq!(doc.revision(), Revision(2));
    doc.redo(&mut state).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "x");
}

#[test]
fn every_small_three_cursor_insert_matches_an_independent_stream_oracle() {
    for length in 0..=7 {
        let text: String = (0..length)
            .map(|index| char::from(b'a' + index as u8))
            .collect();
        for primary in 0..=length {
            for second in 0..=length {
                for third in 0..=length {
                    let positions = std::collections::BTreeSet::from([primary, second, third]);
                    let mut expected = String::new();
                    for position in 0..=length {
                        if positions.contains(&position) {
                            expected.push('Ω');
                        }
                        if let Some(byte) = text.as_bytes().get(position) {
                            expected.push(char::from(*byte));
                        }
                    }
                    let mut doc = document(&text);
                    let mut state = carets(
                        selection(primary, primary),
                        &[selection(second, second), selection(third, third)],
                    );
                    edit::insert(&mut doc, &mut state, "Ω", EditKind::Typing).unwrap();
                    assert_eq!(doc.snapshot().text().as_ref(), expected);
                    assert_eq!(
                        state.cursor().0,
                        primary + positions.range(..=primary).count() * "Ω".len()
                    );
                    assert_eq!(state.secondary.len() + 1, positions.len());
                    doc.undo(&mut state).unwrap();
                    assert_eq!(doc.snapshot().text().as_ref(), text);
                    assert_eq!(state.cursor().0, primary);
                }
            }
        }
    }
}

#[test]
fn search_replacement_from_a_multi_selection_keeps_valid_state_and_exact_undo() {
    let mut doc = document("alpha alpha alpha");
    let mut state = carets(selection(6, 11), &[selection(0, 5), selection(12, 17)]);
    let before = state.clone();
    let snapshot = doc.snapshot();
    let mut results = kode_core::editor::search::SearchResults::default();
    results.publish(
        kode_core::editor::search::scan(&snapshot, "alpha", || false).unwrap(),
        &snapshot,
        "alpha",
    );
    assert_eq!(results.replace(&mut doc, &mut state, "é", true).unwrap(), 3);
    assert_eq!(doc.snapshot().text().as_ref(), "é é é");
    state.validate(&doc.snapshot()).unwrap();
    assert!(state.secondary.is_empty());
    doc.undo(&mut state).unwrap();
    assert_eq!(state, before);
    assert_eq!(doc.snapshot().text().as_ref(), "alpha alpha alpha");
}
