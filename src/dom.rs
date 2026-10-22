//! A minimal reference-counted DOM, built by html5ever.
//!
//! Only what the renderers need: elements with their attributes, text, and
//! the tree structure. Comments, doctypes and processing instructions are
//! kept as nodes (the parser needs somewhere to put them) but carry nothing.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::mem;
use std::rc::{Rc, Weak};

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_document, Attribute, ParseOpts, QualName};

pub type Handle = Rc<Node>;

pub struct Node {
    parent: Cell<Option<Weak<Node>>>,
    pub children: RefCell<Vec<Handle>>,
    pub data: NodeData,
}

pub enum NodeData {
    Document,
    Text {
        contents: RefCell<StrTendril>,
    },
    Element {
        name: QualName,
        attrs: RefCell<Vec<Attribute>>,
        /// The contents of a `<template>`, which are not its children.
        template_contents: Option<Handle>,
    },
    /// A comment, doctype or processing instruction.
    Other,
}

impl Node {
    fn new(data: NodeData) -> Handle {
        Rc::new(Node {
            parent: Cell::new(None),
            children: RefCell::new(Vec::new()),
            data,
        })
    }

    fn parent(&self) -> Option<Handle> {
        let weak = self.parent.take()?;
        let parent = weak.upgrade();
        self.parent.set(Some(weak));
        parent
    }
}

impl Drop for Node {
    // Dropping a deeply nested tree recursively would overflow the stack.
    fn drop(&mut self) {
        let mut stack = mem::take(self.children.get_mut());
        if let NodeData::Element {
            template_contents: Some(contents),
            ..
        } = &mut self.data
        {
            stack.push(contents.clone());
        }
        while let Some(node) = stack.pop() {
            if let Ok(mut node) = Rc::try_unwrap(node) {
                stack.append(node.children.get_mut());
                if let NodeData::Element {
                    template_contents, ..
                } = &mut node.data
                {
                    stack.extend(template_contents.take());
                }
            }
        }
    }
}

pub struct Dom {
    pub document: Handle,
}

/// Parse a UTF-8 document; invalid sequences become U+FFFD. Scripting is
/// off, as for a reader that doesn't run scripts: `<noscript>` holds markup.
pub fn parse(input: &[u8]) -> Dom {
    let text = String::from_utf8_lossy(input);
    let mut opts = ParseOpts::default();
    opts.tree_builder.scripting_enabled = false;
    parse_document(Sink::default(), opts).one(StrTendril::from(&*text))
}

struct Sink {
    document: Handle,
}

impl Default for Sink {
    fn default() -> Self {
        Sink {
            document: Node::new(NodeData::Document),
        }
    }
}

fn append(parent: &Handle, child: Handle) {
    let old = child.parent.replace(Some(Rc::downgrade(parent)));
    debug_assert!(old.is_none());
    parent.children.borrow_mut().push(child);
}

/// Append text to the last child if that is a text node.
fn append_to_last_text(parent: &Handle, text: &str) -> bool {
    match parent.children.borrow().last() {
        Some(last) => match &last.data {
            NodeData::Text { contents } => {
                contents.borrow_mut().push_slice(text);
                true
            }
            _ => false,
        },
        None => false,
    }
}

fn remove_from_parent(target: &Handle) {
    if let Some(parent) = target.parent() {
        target.parent.set(None);
        let mut children = parent.children.borrow_mut();
        if let Some(i) = children.iter().position(|c| Rc::ptr_eq(c, target)) {
            children.remove(i);
        }
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Dom;
    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Dom {
        Dom {
            document: self.document,
        }
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.document.clone()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> &'a QualName {
        match &target.data {
            NodeData::Element { name, .. } => name,
            _ => panic!("not an element"),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        let template_contents = flags.template.then(|| Node::new(NodeData::Document));
        Node::new(NodeData::Element {
            name,
            attrs: RefCell::new(attrs),
            template_contents,
        })
    }

    fn create_comment(&self, _text: StrTendril) -> Handle {
        Node::new(NodeData::Other)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Handle {
        Node::new(NodeData::Other)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        match child {
            NodeOrText::AppendNode(node) => append(parent, node),
            NodeOrText::AppendText(text) => {
                if !append_to_last_text(parent, &text) {
                    append(
                        parent,
                        Node::new(NodeData::Text {
                            contents: RefCell::new(text),
                        }),
                    );
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.parent().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public: StrTendril,
        _system: StrTendril,
    ) {
        append(&self.document, Node::new(NodeData::Other));
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        match &target.data {
            NodeData::Element {
                template_contents: Some(contents),
                ..
            } => contents.clone(),
            _ => panic!("not a template element"),
        }
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        let parent = sibling.parent().expect("sibling has no parent");
        let mut children = parent.children.borrow_mut();
        let i = children
            .iter()
            .position(|c| Rc::ptr_eq(c, sibling))
            .expect("sibling not among its parent's children");
        let node = match child {
            NodeOrText::AppendText(text) => {
                if i > 0 {
                    if let NodeData::Text { contents } = &children[i - 1].data {
                        contents.borrow_mut().push_tendril(&text);
                        return;
                    }
                }
                Node::new(NodeData::Text {
                    contents: RefCell::new(text),
                })
            }
            NodeOrText::AppendNode(node) => {
                drop(children);
                remove_from_parent(&node);
                children = parent.children.borrow_mut();
                node
            }
        };
        node.parent.set(Some(Rc::downgrade(&parent)));
        let i = children
            .iter()
            .position(|c| Rc::ptr_eq(c, sibling))
            .expect("sibling not among its parent's children");
        children.insert(i, node);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if let NodeData::Element {
            attrs: existing, ..
        } = &target.data
        {
            let mut existing = existing.borrow_mut();
            for attr in attrs {
                if !existing.iter().any(|e| e.name == attr.name) {
                    existing.push(attr);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        remove_from_parent(target);
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        let children = mem::take(&mut *node.children.borrow_mut());
        for child in &children {
            child.parent.set(Some(Rc::downgrade(new_parent)));
        }
        new_parent.children.borrow_mut().extend(children);
    }
}
