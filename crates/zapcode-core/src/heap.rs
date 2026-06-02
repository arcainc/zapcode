//! The object heap.
//!
//! `Value::Array` and `Value::Object` carry an integer [`Handle`] into this
//! heap rather than owning their contents inline. Cloning a `Value` copies the
//! handle, so multiple bindings share one backing slot — giving JS reference
//! semantics (aliasing, mutation through a parameter, identity `===`). Because
//! handles are plain indices, the heap serializes as a flat `Vec` that
//! preserves sharing and tolerates cycles for free in snapshots.

use crate::value::Value;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// An index into the [`Heap`]'s slot table identifying one array or object.
pub type Handle = u32;

/// The backing store for one reference value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HeapSlot {
    Array(Vec<Value>),
    Object(IndexMap<Arc<str>, Value>),
}

/// A flat arena of array/object slots. Owned by the VM and carried in snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Heap {
    slots: Vec<HeapSlot>,
}

impl Heap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Allocate an array slot, returning its handle. (Wrap in `Value::Array`.)
    pub fn alloc_array(&mut self, items: Vec<Value>) -> Handle {
        let handle = self.slots.len() as Handle;
        self.slots.push(HeapSlot::Array(items));
        handle
    }

    /// Allocate an object slot, returning its handle. (Wrap in `Value::Object`.)
    pub fn alloc_object(&mut self, fields: IndexMap<Arc<str>, Value>) -> Handle {
        let handle = self.slots.len() as Handle;
        self.slots.push(HeapSlot::Object(fields));
        handle
    }

    /// Borrow the array at `handle` (empty slice if the handle isn't an array).
    pub fn array(&self, handle: Handle) -> &[Value] {
        match self.slots.get(handle as usize) {
            Some(HeapSlot::Array(a)) => a,
            _ => &[],
        }
    }

    /// A cloned copy of the array at `handle` — used by readers that also need
    /// `&mut self` on the VM, to avoid borrowing the heap across the operation.
    pub fn array_vec(&self, handle: Handle) -> Vec<Value> {
        self.array(handle).to_vec()
    }

    pub fn array_mut(&mut self, handle: Handle) -> Option<&mut Vec<Value>> {
        match self.slots.get_mut(handle as usize) {
            Some(HeapSlot::Array(a)) => Some(a),
            _ => None,
        }
    }

    /// Replace the contents of an array slot in place (handle stays valid).
    pub fn set_array(&mut self, handle: Handle, items: Vec<Value>) {
        if let Some(slot) = self.slots.get_mut(handle as usize) {
            *slot = HeapSlot::Array(items);
        }
    }

    /// Borrow the object at `handle`.
    pub fn object(&self, handle: Handle) -> Option<&IndexMap<Arc<str>, Value>> {
        match self.slots.get(handle as usize) {
            Some(HeapSlot::Object(o)) => Some(o),
            _ => None,
        }
    }

    /// A cloned copy of the object at `handle`.
    pub fn object_map(&self, handle: Handle) -> IndexMap<Arc<str>, Value> {
        self.object(handle).cloned().unwrap_or_default()
    }

    pub fn object_mut(&mut self, handle: Handle) -> Option<&mut IndexMap<Arc<str>, Value>> {
        match self.slots.get_mut(handle as usize) {
            Some(HeapSlot::Object(o)) => Some(o),
            _ => None,
        }
    }

    /// Replace the contents of an object slot in place.
    pub fn set_object(&mut self, handle: Handle, fields: IndexMap<Arc<str>, Value>) {
        if let Some(slot) = self.slots.get_mut(handle as usize) {
            *slot = HeapSlot::Object(fields);
        }
    }

    /// True if the handle refers to an array slot.
    pub fn is_array(&self, handle: Handle) -> bool {
        matches!(self.slots.get(handle as usize), Some(HeapSlot::Array(_)))
    }
}
