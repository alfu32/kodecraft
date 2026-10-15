use super::*;
use crate::panels::git::GitWork;
impl AppRuntime {
    pub(super) fn git_work(&mut self, work: GitWork) -> Result<()> {
        match work {
            GitWork::Editor { owner, work } => self.editor_work(owner, work),
            GitWork::Clipboard { tag, job, read } => self.clipboard_work(
                tag,
                job,
                if read {
                    ClipboardTarget::Git
                } else {
                    ClipboardTarget::None
                },
            ),
            GitWork::Read { tag, kind, job } => {
                let operation = kind.operation();
                let diff = matches!(kind, crate::panels::git::ReadKind::Diff);
                self.blocking(
                    tag,
                    self.state.git.owner(),
                    operation,
                    false,
                    Context::GitRead(kind),
                    Box::new(move |cancel| {
                        let report = job.run(&cancel)?;
                        if diff {
                            match report {
                                kode_git::read::ReadOutput::Diff(contents) => {
                                    kode_git::diff::DiffPlan::capture(&job.root, contents, &cancel)
                                        .map(|plan| Output::GitDiff(Arc::new(plan)))
                                }
                                _ => Err(DomainError::Unsupported(
                                    "expected Git diff contents".into(),
                                )),
                            }
                        } else {
                            Ok(Output::GitRead(Box::new(report)))
                        }
                    }),
                )
            }
            GitWork::Write { tag, root, command } => {
                let checkout = command.changes_worktree();
                let repository_root = self
                    .state
                    .git
                    .read
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.location.work_tree.clone())
                    .unwrap_or_else(|| root.clone());
                let protected_paths = [
                    self.state.project.root.join(".kode-rs.json"),
                    self.state.project.root.join(".kode-rs"),
                ]
                .into_iter()
                .filter_map(|path| {
                    path.strip_prefix(&repository_root)
                        .ok()
                        .map(std::path::Path::to_path_buf)
                })
                .collect::<Vec<_>>();
                if checkout {
                    let dirty = self.state.documents.ids().any(|id| {
                        self.state.documents.document(id).is_ok_and(|document| {
                            matches!(document.origin(), DocumentOrigin::File(_))
                                && document.is_dirty()
                        })
                    }) || self.state.hexes.values().any(|view| view.modified);
                    let validation = if dirty {
                        Err(DomainError::Git {
                            operation: "checkout branch".into(),
                            message: "save or preserve dirty editor buffers before checkout".into(),
                        })
                    } else if self.state.persistence.is_saving() {
                        Err(DomainError::Git {
                            operation: "checkout branch".into(),
                            message: "wait for workspace persistence before checkout".into(),
                        })
                    } else {
                        let affected = self
                            .state
                            .git
                            .read
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.location.work_tree.clone())
                            .unwrap_or_else(|| root.clone());
                        self.state
                            .documents
                            .reserve_file_action(tag, vec![affected])
                    };
                    if let Err(error) = validation {
                        self.state.git.complete_write(
                            tag,
                            Err(error),
                            &mut self.state.documents,
                        )?;
                        self.state.notice = self.state.git.message.clone();
                        return Ok(());
                    }
                }
                self.blocking(
                    tag,
                    self.state.git.owner(),
                    "git-write",
                    true,
                    Context::GitWrite(checkout),
                    Box::new(move |cancel| {
                        command
                            .run_protecting(root, &cancel, &protected_paths)
                            .map(Output::GitWritten)
                    }),
                )
            }
        }
    }
    pub(super) fn git_written(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        checkout: bool,
        result: Result<Output>,
    ) -> Result<()> {
        if checkout {
            self.state.documents.finish_file_action(tag, None)?;
        }
        if owner != self.state.git.owner() {
            return Ok(());
        }
        let result = result.and_then(|output| match output {
            Output::GitWritten(receipt) => Ok(receipt),
            _ => Err(DomainError::Unsupported("wrong Git write output".into())),
        });
        if result
            .as_ref()
            .is_ok_and(|receipt| receipt.worktree_changed)
        {
            self.state.clear_diff()?;
            self.reload_after_checkout()?;
        }
        self.state
            .git
            .complete_write(tag, result, &mut self.state.documents)?;
        self.state.notice = self.state.git.message.clone();
        self.state.changed()?;
        Ok(())
    }
    /// Preserve dirty buffers, invalidate clean captures and load the active file
    /// afresh without changing the user's focus. Reopening hidden files also
    /// bypasses session bytes from the previous branch once they were clean.
    fn reload_after_checkout(&mut self) -> Result<()> {
        self.reload_after_change(None)
    }
    fn reload_after_change(&mut self, only_path: Option<&std::path::Path>) -> Result<()> {
        let repository_root = self
            .state
            .git
            .read
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.location.work_tree.clone())
            .unwrap_or_else(|| self.state.project.root.clone());
        let pending_automatic = self
            .state
            .pending_open
            .is_some_and(|tag| self.git_reload_tags.contains(&tag));
        let pending = self.state.pending_open.and_then(|tag| {
            self.open_requests.get(&tag).cloned().or_else(|| {
                self.state.work.iter().find_map(|work| match work {
                    AppWork::Open {
                        tag: current,
                        request,
                        destination,
                    } if *current == tag => Some((*destination, request.clone())),
                    _ => None,
                })
            })
        });
        let active = self.state.editor.as_ref().and_then(|view| {
            self.state
                .documents
                .document(view.document_id())
                .ok()
                .map(|document| (document.origin().clone(), view.input.state.caret.cursor()))
        });
        let search_document = self
            .state
            .search
            .as_ref()
            .and_then(|dialog| dialog.editor.as_ref())
            .map(|view| view.document_id());
        let mut clean = vec![];
        for id in self.state.documents.ids() {
            let document = self.state.documents.document(id)?;
            if Some(id) != search_document
                && matches!(document.origin(), DocumentOrigin::File(path) if path.starts_with(&repository_root))
                && matches!(document.origin(), DocumentOrigin::File(path) if only_path.is_none_or(|changed| path == changed))
                && !document.is_dirty()
                && !self.state.documents.save_in_progress(id)
            {
                clean.push((id, document.origin().clone()));
            }
        }
        if self
            .state
            .editor
            .as_ref()
            .is_some_and(|view| clean.iter().any(|(id, _)| *id == view.document_id()))
        {
            self.state.cache_main_view()?;
            if let Some(mut view) = self.state.editor.take() {
                view.detach(&mut self.state.focus);
                let owner = view.owner();
                self.state.work.extend(
                    view.take_work()
                        .into_iter()
                        .map(|work| AppWork::Editor { owner, work }),
                );
            }
        }
        for (id, origin) in clean {
            self.state.documents.remove(id)?;
            self.state.views.remove(&id);
            if let DocumentOrigin::File(path) = origin {
                self.git_fresh_paths.insert(path.clone());
                if let Some(service) = &mut self.intelligence
                    && let Some(path) = path.to_str()
                {
                    service.remove_path(path)?;
                }
            }
        }
        let mut active_binary = None;
        if let Some(path) = self.state.active_hex.clone()
            && path.starts_with(&repository_root)
            && only_path.is_none_or(|changed| path == changed)
            && self
                .state
                .hexes
                .get(&path)
                .is_some_and(|view| !view.modified)
        {
            if let Some(view) = self.state.hexes.get_mut(&path) {
                view.detach(&mut self.state.focus);
            }
            self.state.active_hex = None;
            active_binary = Some(path);
        }
        self.state.hexes.retain(|path, view| {
            if path.starts_with(&repository_root)
                && only_path.is_none_or(|changed| path == changed)
                && !view.modified
            {
                self.git_fresh_paths.insert(path.clone());
                false
            } else {
                true
            }
        });
        if let Some(path) = self.state.image.as_ref().map(|view| view.path.clone())
            && path.starts_with(&repository_root)
            && only_path.is_none_or(|changed| path == changed)
        {
            self.git_fresh_paths.insert(path.clone());
            active_binary = Some(path);
            self.state.detach_image();
        }
        self.state.files.refresh()?;
        if let Some(service) = &mut self.intelligence {
            service.rescan(self.state.project.clone(), false)?;
            self.native_auto_scan = true;
        }
        let mut preserve_focus = true;
        let request = if let Some((OpenDestination::Main, mut request)) = pending {
            preserve_focus = pending_automatic;
            if self.git_fresh_paths.contains(&request.path) {
                request.saved = None;
            }
            Some(request)
        } else if let Some((DocumentOrigin::File(path), position)) = active {
            if self.git_fresh_paths.contains(&path) {
                Some(OpenRequest {
                    path: path.clone(),
                    saved: None,
                    navigation: Some(kode_core::intelligence::NavigationTarget {
                        path,
                        document: None,
                        identifier_range: kode_core::TextRange::caret(position),
                        location: None,
                        declaration_range: None,
                        language: None,
                        kind: None,
                        name: None,
                        missing_source: false,
                    }),
                })
            } else {
                None
            }
        } else {
            active_binary.map(|path| OpenRequest {
                path,
                saved: None,
                navigation: None,
            })
        };
        if let Some(request) = request {
            if preserve_focus {
                self.state.open_background(request)?;
            } else {
                self.state.open(request, OpenDestination::Main)?;
            }
            if preserve_focus && let Some(tag) = self.state.pending_open {
                self.git_reload_tags.insert(tag);
            }
        }
        Ok(())
    }
    pub(super) fn diff_work(&mut self, work: crate::panels::git::diff::DiffWork) -> Result<()> {
        use crate::panels::git::diff::{DiffWork, projection::Projection};
        match work {
            DiffWork::Editor { owner, work } => self.editor_work(owner, work),
            DiffWork::Layout {
                tag,
                owner,
                plan,
                key,
            } => self.blocking(
                tag,
                owner,
                "diff-layout",
                false,
                Context::DiffLayout,
                Box::new(move |cancel| {
                    Projection::build(&plan, key.width, key.context, key.unified, &cancel)
                        .map(Output::DiffLayout)
                }),
            ),
            DiffWork::Restore {
                tag,
                owner,
                job,
                source_revision,
            } => {
                let path = job.root.join(&job.plan.contents().path);
                let dirty = self.state.documents.ids().any(|id| {
                    self.state.documents.document(id).is_ok_and(|document| {
                        document.origin() == &DocumentOrigin::File(path.clone())
                            && document.is_dirty()
                    })
                }) || self
                    .state
                    .hexes
                    .get(&path)
                    .is_some_and(|view| view.modified);
                let protected = path == self.state.project.root.join(".kode-rs.json")
                    || path.starts_with(self.state.project.root.join(".kode-rs"));
                let stale_document = source_revision.is_some_and(|(id, revision)| {
                    !self
                        .state
                        .documents
                        .document(id)
                        .is_ok_and(|document| document.revision() == revision)
                });
                let validation = if dirty || protected || stale_document {
                    Err(DomainError::Git {
                        operation: "restore chunk".into(),
                        message: if protected {
                            "close this workspace before modifying its session/cache files"
                        } else if stale_document {
                            "open document changed since this preview; refresh the comparison"
                        } else {
                            "save or preserve the dirty open buffer before restoring a chunk"
                        }
                        .into(),
                    })
                } else {
                    self.state.documents.reserve_file_action(tag, vec![path])
                };
                if let Err(error) = validation {
                    if let Some(diff) = &mut self.state.diff
                        && diff.owner() == owner
                    {
                        diff.restored(tag, error.to_string());
                    }
                    return Ok(());
                }
                self.blocking(
                    tag,
                    owner,
                    "diff-restore",
                    true,
                    Context::DiffRestore,
                    Box::new(move |cancel| job.run(&cancel).map(Output::DiffRestored)),
                )
            }
        }
    }
    pub(super) fn diff_restored(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        result: Result<Output>,
    ) -> Result<()> {
        self.state.documents.finish_file_action(tag, None)?;
        let result = result.and_then(|output| match output {
            Output::DiffRestored(receipt) => Ok(receipt),
            _ => Err(DomainError::Unsupported(
                "wrong chunk restore output".into(),
            )),
        });
        let message = result
            .as_ref()
            .map_or_else(ToString::to_string, |_| "Restored captured chunk".into());
        let active = self
            .state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.owner() == owner);
        if let Some(diff) = &mut self.state.diff
            && active
        {
            diff.restored(tag, message.clone());
        }
        self.state.notice = message;
        if let Ok(receipt) = result {
            self.reload_after_change(Some(&receipt.path))?;
            self.state.git.request_refresh();
            if active && let Some(diff) = &self.state.diff {
                self.state
                    .git
                    .queue_diff(kode_git::read::ReadRequest::Diff {
                        path: diff.plan.contents().path.clone(),
                        staged: false,
                    });
            }
            self.state.changed()?;
        }
        Ok(())
    }
}
