//! The stack of open elements.
//!
//! Wraps the plain vector with enough bookkeeping to answer "does the stack
//! have a `p` element in button scope?" without walking the stack. That check
//! runs for most block-level start tags, so walking the stack made deeply
//! nested documents take quadratic time.

use std::ops::Deref;

/// What an open element is, as far as the button-scope check cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ElemKind {
    /// An HTML `p` element.
    P,
    /// An element in the button scope set (which ends the search).
    ButtonScopeBoundary,
    Other,
}

pub(crate) struct OpenElems<Handle> {
    elems: Vec<Handle>,
    kinds: Vec<ElemKind>,
    /// Indices of `p` elements in `elems`, in increasing order.
    p_indices: Vec<usize>,
    /// Indices of button scope boundaries in `elems`, in increasing order.
    boundary_indices: Vec<usize>,
}

impl<Handle> Default for OpenElems<Handle> {
    fn default() -> Self {
        OpenElems {
            elems: Vec::new(),
            kinds: Vec::new(),
            p_indices: Vec::new(),
            boundary_indices: Vec::new(),
        }
    }
}

impl<Handle> Deref for OpenElems<Handle> {
    type Target = [Handle];

    fn deref(&self) -> &[Handle] {
        &self.elems
    }
}

impl<Handle> OpenElems<Handle> {
    pub(crate) fn push(&mut self, elem: Handle, kind: ElemKind) {
        let index = self.elems.len();
        self.elems.push(elem);
        self.kinds.push(kind);
        match kind {
            ElemKind::P => self.p_indices.push(index),
            ElemKind::ButtonScopeBoundary => self.boundary_indices.push(index),
            ElemKind::Other => {},
        }
    }

    pub(crate) fn pop(&mut self) -> Option<Handle> {
        let elem = self.elems.pop()?;
        self.kinds.pop();
        self.forget_indices_from(self.elems.len());
        Some(elem)
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.elems.truncate(len);
        self.kinds.truncate(len);
        self.forget_indices_from(len);
    }

    /// Remove all the elements, returning them.
    pub(crate) fn drain_all(&mut self) -> std::vec::Drain<'_, Handle> {
        self.kinds.clear();
        self.p_indices.clear();
        self.boundary_indices.clear();
        self.elems.drain(..)
    }

    // Removing, inserting or replacing in the middle of the stack only happens
    // in the adoption agency algorithm and `remove_from_stack`, which already
    // walk the stack, so recomputing the indices doesn't change their cost.

    pub(crate) fn remove(&mut self, index: usize) -> Handle {
        let elem = self.elems.remove(index);
        self.kinds.remove(index);
        self.reindex();
        elem
    }

    pub(crate) fn insert(&mut self, index: usize, elem: Handle, kind: ElemKind) {
        self.elems.insert(index, elem);
        self.kinds.insert(index, kind);
        self.reindex();
    }

    pub(crate) fn replace(&mut self, index: usize, elem: Handle, kind: ElemKind) -> Handle {
        self.kinds[index] = kind;
        self.reindex();
        std::mem::replace(&mut self.elems[index], elem)
    }

    /// Equivalent to searching down from the current node for a `p` element,
    /// stopping at the first button scope boundary.
    pub(crate) fn has_p_in_button_scope(&self) -> bool {
        match (self.p_indices.last(), self.boundary_indices.last()) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(p), Some(boundary)) => p > boundary,
        }
    }

    fn forget_indices_from(&mut self, len: usize) {
        while self.p_indices.last().is_some_and(|&i| i >= len) {
            self.p_indices.pop();
        }
        while self.boundary_indices.last().is_some_and(|&i| i >= len) {
            self.boundary_indices.pop();
        }
    }

    fn reindex(&mut self) {
        self.p_indices.clear();
        self.boundary_indices.clear();
        for (index, kind) in self.kinds.iter().enumerate() {
            match kind {
                ElemKind::P => self.p_indices.push(index),
                ElemKind::ButtonScopeBoundary => self.boundary_indices.push(index),
                ElemKind::Other => {},
            }
        }
    }
}
