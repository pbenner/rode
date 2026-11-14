//! Explicit editor operations admitted after native input, before project transitions.
//!
//! Views retain their widget/replay state. This coordinator owns operations that
//! change a document's presentation or native host; workers never borrow a view.
use super::*;
use std::collections::VecDeque;

const MAX_PENDING: usize = 512;

pub(super) struct Position {
    pub row: usize,
    pub column: usize,
    pub version: u64,
}

pub(super) enum Template {
    Source,
    Notebook,
    Presentation,
}

pub(super) enum Operation {
    Select {
        document: u64,
        position: Option<Position>,
    },
    Opened {
        path: String,
        viewport: egui::ViewportId,
        response: Box<Response>,
    },
    Move {
        document: u64,
        destination: Option<egui::ViewportId>,
    },
    NewWindow,
    Create {
        viewport: egui::ViewportId,
        template: Template,
    },
    Return(egui::ViewportId),
}

impl Operation {
    fn workspace_bound(&self) -> bool {
        !matches!(self, Self::NewWindow | Self::Return(_))
    }
}

#[derive(Default)]
pub(super) struct Coordinator {
    collecting: bool,
    pending: VecDeque<(u64, Operation)>,
}
impl Coordinator {
    pub(super) fn collecting(&self) -> bool {
        self.collecting
    }
    fn push(&mut self, generation: u64, operation: Operation) -> Result<(), Operation> {
        if let Operation::Return(viewport) = &operation
            && self
                .pending
                .iter()
                .any(|(_, op)| matches!(op, Operation::Return(id) if id == viewport))
        {
            return Ok(());
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(operation);
        }
        self.pending.push_back((generation, operation));
        Ok(())
    }
}

impl EditorApp {
    /// Native editor commands name their host; tool and root menus use the last editor focus.
    pub(super) fn command_editor(&self, ctx: &egui::Context) -> egui::ViewportId {
        let viewport = ctx.viewport_id();
        if viewport != egui::ViewportId::ROOT && self.editors.contains(viewport) {
            viewport
        } else {
            self.focused_editor()
        }
    }

    pub(super) fn begin_editor_frame(&mut self) {
        self.editor_operations.collecting = true;
    }

    /// One commit phase follows every root, tool and editor input callback.
    pub(super) fn finish_editor_frame(&mut self, ctx: &egui::Context) {
        self.editor_operations.collecting = false;
        self.apply_editor_operations(ctx);
        self.advance_guarded_action(ctx);
    }

    pub(super) fn submit_editor_operation(&mut self, operation: Operation, ctx: &egui::Context) {
        if self.editor_operations.collecting() || self.editor_input_pending(ctx) {
            if let Err(operation) = self
                .editor_operations
                .push(self.workers.generation(), operation)
            {
                if let Operation::Opened { path, .. } = operation {
                    self.documents.finish_open(&path);
                }
                self.notify(
                    "Wait for queued editor operations before submitting another action.",
                    true,
                );
            }
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        } else {
            self.execute_editor_operation(operation, ctx);
        }
    }

    pub(super) fn apply_editor_operations(&mut self, ctx: &egui::Context) {
        if self.editor_operations.collecting() {
            return;
        }
        if self.editor_input_pending(ctx) {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            return;
        }
        while let Some((generation, operation)) = self.editor_operations.pending.pop_front() {
            if operation.workspace_bound() && generation != self.workers.generation() {
                continue;
            }
            self.execute_editor_operation(operation, ctx);
        }
    }

    fn execute_editor_operation(&mut self, operation: Operation, ctx: &egui::Context) {
        match operation {
            Operation::Select { document, position } => {
                if let Some(position) = position {
                    let Some(doc) = self
                        .documents
                        .by_id_mut(document)
                        .filter(|doc| doc.version() == position.version)
                    else {
                        return;
                    };
                    let offset = doc
                        .text
                        .split_inclusive('\n')
                        .take(position.row.saturating_sub(1))
                        .map(|line| line.chars().count())
                        .sum::<usize>()
                        + position.column.saturating_sub(1);
                    doc.selection = Some((offset, offset));
                }
                if self.documents.select(document) {
                    self.reveal_tab(document);
                    if let Some(viewport) = self.documents.owner(document) {
                        self.focus_editor_in(viewport, ctx);
                    }
                }
            }
            Operation::Opened {
                path,
                viewport,
                response,
            } => {
                let viewport = self.documents.finish_open(&path).unwrap_or(viewport);
                let viewport = if self.editors.contains(viewport) {
                    viewport
                } else {
                    egui::ViewportId::ROOT
                };
                let id = self.documents.open_in(
                    viewport,
                    *response,
                    &self.markdown_images,
                    &self.notebook_shutdowns,
                );
                if self.documents.by_id(id).is_some_and(|doc| {
                    crate::languages::detect(doc.path.as_deref())
                        .is_some_and(|language| language.id == "python")
                }) {
                    self.refresh_environment(false, ctx);
                }
                self.status = "File loaded".into();
                if self.navigation.accepts_editor_focus() {
                    self.execute_editor_operation(
                        Operation::Select {
                            document: id,
                            position: None,
                        },
                        ctx,
                    );
                }
            }
            Operation::Move {
                document,
                destination,
            } => self.transfer_editor_tab(document, destination, ctx),
            Operation::NewWindow => {
                let viewport = self.allocate_editor_window();
                self.focus_editor_in(viewport, ctx);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Operation::Create { viewport, template } => {
                let viewport = if self.editors.contains(viewport) {
                    viewport
                } else {
                    egui::ViewportId::ROOT
                };
                let document = match template {
                    Template::Source => Some(self.documents.new_file_in(viewport)),
                    Template::Notebook => self.new_notebook_in(viewport),
                    Template::Presentation => self.new_marp_in(viewport),
                };
                if let Some(document) = document {
                    self.execute_editor_operation(
                        Operation::Select {
                            document,
                            position: None,
                        },
                        ctx,
                    );
                }
            }
            Operation::Return(viewport) => {
                if viewport != egui::ViewportId::ROOT && self.editors.contains(viewport) {
                    self.reattach_editor(viewport, ctx);
                }
            }
        }
    }
}
