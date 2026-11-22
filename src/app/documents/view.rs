//! Mutually exclusive document representations. Source previews are adjuncts
//! of text; they cannot coexist with a binary, image, paged or notebook editor.
use super::*;

/// The source buffer remains authoritative while Markdown or Marp is displayed.
#[derive(Default)]
pub(in crate::app) struct SourceViews {
    pub markdown: Option<markdown::Preview>,
    pub marp: Option<marp_view::View>,
}

/// One primary representation per tab. Large native viewers are boxed so an
/// ordinary source tab does not reserve space for every supported document kind.
pub(super) enum Representation {
    Source(Box<SourceViews>),
    Binary(Box<binary_view::HexView>),
    Image(Box<image_view::ImageView>),
    Paged(Box<pdf_view::PdfView>),
    /// The empty slot exists only while the UI temporarily borrows the boxed
    /// notebook together with document metadata. Its primary kind stays stable.
    Notebook(Option<Box<notebook_view::View>>),
}

impl Default for Representation {
    fn default() -> Self {
        Self::Source(Box::default())
    }
}

impl Document {
    fn refresh(&self) -> Option<Refresh> {
        let path = self.path.clone()?;
        Some(match &self.view {
            Representation::Source(_) => Refresh::Read(Box::new(Request::Read { path })),
            Representation::Notebook(_) => Refresh::Read(Box::new(Request::ReadAuto { path })),
            Representation::Binary(_) => Refresh::Read(Box::new(Request::ReadHex { path })),
            Representation::Image(_) => Refresh::Image,
            Representation::Paged(_) => Refresh::Paged,
        })
    }
    #[cfg(test)]
    pub(in crate::app) fn set_binary_fixture(&mut self, view: binary_view::HexView) {
        self.view = Representation::Binary(Box::new(view));
    }
    #[cfg(test)]
    pub(in crate::app) fn set_pdf_fixture(&mut self, view: pdf_view::PdfView) {
        self.view = Representation::Paged(Box::new(view));
    }
    #[cfg(test)]
    pub(in crate::app) fn set_image_fixture(&mut self, view: image_view::ImageView) {
        self.view = Representation::Image(Box::new(view));
    }
    pub(in crate::app) fn source_views_mut(&mut self) -> Option<&mut SourceViews> {
        match &mut self.view {
            Representation::Source(views) => Some(views),
            _ => None,
        }
    }
    pub(in crate::app) fn markdown(&self) -> Option<&markdown::Preview> {
        match &self.view {
            Representation::Source(views) => views.markdown.as_ref(),
            _ => None,
        }
    }
    pub(in crate::app) fn markdown_mut(&mut self) -> Option<&mut markdown::Preview> {
        self.source_views_mut()?.markdown.as_mut()
    }
    pub(in crate::app) fn marp(&self) -> Option<&marp_view::View> {
        match &self.view {
            Representation::Source(views) => views.marp.as_ref(),
            _ => None,
        }
    }
    pub(in crate::app) fn marp_mut(&mut self) -> Option<&mut marp_view::View> {
        self.source_views_mut()?.marp.as_mut()
    }
    pub(in crate::app) fn hex(&self) -> Option<&binary_view::HexView> {
        match &self.view {
            Representation::Binary(view) => Some(view),
            _ => None,
        }
    }
    /// Byte mutations still pass through `edit_binary` to publish dirty/version facts.
    pub(super) fn hex_mut(&mut self) -> Option<&mut binary_view::HexView> {
        match &mut self.view {
            Representation::Binary(view) => Some(view),
            _ => None,
        }
    }
    pub(in crate::app) fn image(&self) -> Option<&image_view::ImageView> {
        match &self.view {
            Representation::Image(view) => Some(view),
            _ => None,
        }
    }
    pub(in crate::app) fn image_mut(&mut self) -> Option<&mut image_view::ImageView> {
        match &mut self.view {
            Representation::Image(view) => Some(view),
            _ => None,
        }
    }
    /// Route shared image zoom to the visible image, paged reader or slide preview.
    pub(in crate::app) fn preview_image_mut(&mut self) -> Option<&mut image_view::ImageView> {
        match &mut self.view {
            Representation::Image(view) => Some(view),
            Representation::Paged(view) => Some(&mut view.image),
            Representation::Source(views) => views
                .marp
                .as_mut()
                .filter(|view| view.showing)
                .and_then(|view| view.pdf.as_mut())
                .map(|pdf| &mut pdf.image),
            _ => None,
        }
    }
    pub(in crate::app) fn pdf(&self) -> Option<&pdf_view::PdfView> {
        match &self.view {
            Representation::Paged(view) => Some(view),
            _ => None,
        }
    }
    pub(in crate::app) fn pdf_mut(&mut self) -> Option<&mut pdf_view::PdfView> {
        match &mut self.view {
            Representation::Paged(view) => Some(view),
            _ => None,
        }
    }
    pub(in crate::app) fn notebook(&self) -> Option<&notebook_view::View> {
        match &self.view {
            Representation::Notebook(view) => view.as_deref(),
            _ => None,
        }
    }
    pub(in crate::app) fn notebook_mut(&mut self) -> Option<&mut notebook_view::View> {
        match &mut self.view {
            Representation::Notebook(view) => view.as_deref_mut(),
            _ => None,
        }
    }
    /// Install parsed/new notebook state, dropping the previous representation
    /// through its own cancellation/shutdown owner.
    pub(in crate::app) fn replace_notebook(&mut self, view: impl Into<Box<notebook_view::View>>) {
        self.view = Representation::Notebook(Some(view.into()));
    }
    /// Temporarily separate a structured editor from its source/save metadata
    /// while it synchronizes edits. The caller restores it before returning.
    pub(in crate::app) fn take_notebook(&mut self) -> Option<Box<notebook_view::View>> {
        match &mut self.view {
            Representation::Notebook(view) => view.take(),
            _ => None,
        }
    }

    /// Whether source keyboard actions target the visible editor, rather than
    /// a preview or structured notebook. All input routes share this decision.
    pub(in crate::app) fn source_visible(&self) -> bool {
        match &self.view {
            Representation::Source(views) => {
                !views.markdown.as_ref().is_some_and(|view| view.showing)
                    && !views.marp.as_ref().is_some_and(|view| view.showing)
            }
            Representation::Notebook(view) => view.as_ref().is_some_and(|view| view.source_mode()),
            _ => false,
        }
    }

    /// Rendered previews share close/zoom keyboard routing. Binary and notebook
    /// editors retain their own structured input handling.
    pub(in crate::app) fn rendered_preview(&self) -> bool {
        match &self.view {
            Representation::Source(_) => !self.source_visible(),
            Representation::Image(_) | Representation::Paged(_) => true,
            Representation::Binary(_) | Representation::Notebook(_) => false,
        }
    }

    /// Only image and paged readers lack an editable source representation.
    pub(in crate::app) fn read_only(&self) -> bool {
        matches!(
            self.view,
            Representation::Image(_) | Representation::Paged(_)
        )
    }

    /// Extension ownership follows the representation, independently of filename
    /// changes or the editor/preview mode currently selected inside that view.
    pub(in crate::app) fn feature(&self) -> Option<crate::extensions::Feature> {
        use crate::extensions::Feature;
        match &self.view {
            Representation::Source(_) => None,
            Representation::Binary(_) => Some(Feature::Binary),
            Representation::Image(_) => Some(Feature::Images),
            Representation::Paged(_) => self.paged_feature(),
            Representation::Notebook(_) => Some(Feature::Notebooks),
        }
    }
}

impl Documents {
    /// Capture refresh intents once per clean, bound, enabled document. Disabled
    /// viewers retain their state rather than being reclassified by a text read.
    pub(in crate::app) fn refreshes(
        &self,
        enabled: impl Fn(crate::extensions::Feature) -> bool,
    ) -> Vec<(u64, u64, Refresh)> {
        self.items
            .iter()
            .filter(|doc| !doc.dirty() && !doc.saving && !doc.preview_loading())
            .filter(|doc| doc.feature().is_none_or(&enabled))
            .filter_map(|doc| Some((doc.id, doc.version(), doc.refresh()?)))
            .collect()
    }
}
