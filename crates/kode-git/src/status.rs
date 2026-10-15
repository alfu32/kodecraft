use crate::{
    model::*,
    repository::{check, error, path},
};
use git2::{Repository, Status, StatusOptions};
use kode_core::Result;
use kode_platform::jobs::CancellationToken;
pub(crate) fn read(
    repo: &Repository,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Listing<StatusEntry>> {
    check(cancel)?;
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .include_unmodified(false)
        .renames_head_to_index(false)
        .renames_index_to_workdir(false)
        .update_index(false)
        .no_refresh(true)
        .include_unreadable(true)
        .sort_case_sensitively(true);
    let statuses = repo
        .statuses(Some(&mut options))
        .map_err(|failure| error(repo.path(), "read status", failure))?;
    check(cancel)?;
    let mut conflicts = std::collections::HashMap::new();
    if statuses.iter().any(|entry| entry.status().is_conflicted()) {
        let index = repo
            .index()
            .map_err(|failure| error(repo.path(), "read conflict index", failure))?;
        for conflict in index
            .conflicts()
            .map_err(|failure| error(repo.path(), "list conflict stages", failure))?
        {
            check(cancel)?;
            let conflict =
                conflict.map_err(|failure| error(repo.path(), "read conflict stages", failure))?;
            let code = match (
                conflict.ancestor.is_some(),
                conflict.our.is_some(),
                conflict.their.is_some(),
            ) {
                (false, true, true) => "AA",
                (true, false, false) => "DD",
                (false, true, false) => "AU",
                (false, false, true) => "UA",
                (true, true, false) => "UD",
                (true, false, true) => "DU",
                _ => "UU",
            };
            for entry in [conflict.ancestor, conflict.our, conflict.their]
                .into_iter()
                .flatten()
            {
                conflicts.insert(path(&entry.path)?, code);
            }
        }
    }
    let mut entries = vec![];
    let mut truncated = false;
    for item in statuses.iter() {
        check(cancel)?;
        let state = item.status();
        let name = path(item.path_bytes())?;
        let mut add = |layer, kind, code: &str| {
            if entries.len() == limits.entries {
                truncated = true;
                return;
            }
            entries.push(StatusEntry {
                path: name.clone(),
                original_path: None,
                layer,
                kind,
                code: code.into(),
            });
        };
        if state.is_conflicted() {
            add(
                StatusLayer::Conflict,
                ChangeKind::Conflicted,
                conflicts.get(&name).copied().unwrap_or("UU"),
            );
            if truncated {
                break;
            }
            continue;
        }
        for (flag, kind, code) in [
            (Status::INDEX_NEW, ChangeKind::Added, "A"),
            (Status::INDEX_DELETED, ChangeKind::Deleted, "D"),
            (Status::INDEX_RENAMED, ChangeKind::Renamed, "R"),
            (Status::INDEX_TYPECHANGE, ChangeKind::TypeChanged, "T"),
            (Status::INDEX_MODIFIED, ChangeKind::Modified, "M"),
        ] {
            if state.contains(flag) {
                add(StatusLayer::Index, kind, code);
                break;
            }
        }
        for (flag, kind, code) in [
            (Status::WT_NEW, ChangeKind::Untracked, "??"),
            (Status::WT_DELETED, ChangeKind::Deleted, "D"),
            (Status::WT_RENAMED, ChangeKind::Renamed, "R"),
            (Status::WT_TYPECHANGE, ChangeKind::TypeChanged, "T"),
            (Status::WT_MODIFIED, ChangeKind::Modified, "M"),
            (Status::WT_UNREADABLE, ChangeKind::Unreadable, "!"),
        ] {
            if state.contains(flag) {
                add(StatusLayer::Worktree, kind, code);
                break;
            }
        }
        if truncated {
            break;
        }
    }
    entries.sort_by(|a, b| (&a.path, a.layer).cmp(&(&b.path, b.layer)));
    Ok(Listing { entries, truncated })
}
