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
use std::collections::HashMap;
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

    /// Estimated live byte footprint of the arena: element/entry counts ×
    /// `size_of::<Value>()`, consistent with how `ResourceTracker` charges
    /// allocations. Used to reset `memory_bytes` to a live figure after
    /// in-run compaction (see `docs/in-run-memory-design.md`).
    pub(crate) fn byte_estimate(&self) -> usize {
        let vsz = std::mem::size_of::<Value>();
        self.slots
            .iter()
            .map(|slot| match slot {
                HeapSlot::Array(items) => items.len().saturating_mul(vsz),
                HeapSlot::Object(map) => map.len().saturating_mul(vsz),
            })
            .sum()
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

    /// Append every slot from `other` into this heap, rebasing the handles those
    /// slots carry by the offset where they land. Returns the offset (the index
    /// of `other`'s old handle 0 in this heap), so the caller can rebase any
    /// top-level `Value` handles that referenced `other` with
    /// [`Self::rebase_handles`].
    ///
    /// Used at the host boundary: a binding builds compound inputs / resume
    /// values into a standalone heap (handles `0..n`), then merges that heap into
    /// the live VM heap which already holds builtin and user slots.
    pub fn absorb(&mut self, other: Heap) -> Handle {
        let offset = self.slots.len() as Handle;
        for slot in other.slots {
            let rebased = match slot {
                HeapSlot::Array(items) => HeapSlot::Array(
                    items
                        .into_iter()
                        .map(|v| Self::rebase_handles(v, offset))
                        .collect(),
                ),
                HeapSlot::Object(fields) => HeapSlot::Object(
                    fields
                        .into_iter()
                        .map(|(k, v)| (k, Self::rebase_handles(v, offset)))
                        .collect(),
                ),
            };
            self.slots.push(rebased);
        }
        offset
    }

    /// Add `offset` to the handle of an `Array`/`Object` value (recursively for
    /// any nested handles). Other value kinds are returned unchanged. Pairs with
    /// [`Self::absorb`] to rebase values that referenced the absorbed heap.
    pub fn rebase_handles(value: Value, offset: Handle) -> Value {
        match value {
            Value::Array(h) => Value::Array(h + offset),
            Value::Object(h) => Value::Object(h + offset),
            other => other,
        }
    }

    /// Recursively copy a value into fresh heap slots (independent of the
    /// original), for `structuredClone` and other deep-copy semantics.
    ///
    /// Reference values can alias and form cycles (`const a = []; a.push(a)`), so
    /// a naive recursion would loop forever / overflow the native stack and abort
    /// the host process. `seen` maps each already-cloned *source* handle to its
    /// freshly-allocated *clone* handle: revisiting a source handle reuses its
    /// clone, which preserves shared structure and makes cyclic input terminate
    /// (matching `structuredClone`, which round-trips cycles). To make a cycle
    /// resolvable we must register the clone handle *before* descending into the
    /// children, so a back-edge to the node currently being cloned finds it.
    pub fn deep_clone(&mut self, value: &Value) -> crate::error::Result<Value> {
        let mut seen: HashMap<Handle, Handle> = HashMap::new();
        self.deep_clone_inner(value, &mut seen, 0)
    }

    fn deep_clone_inner(
        &mut self,
        value: &Value,
        seen: &mut HashMap<Handle, Handle>,
        depth: usize,
    ) -> crate::error::Result<Value> {
        // Defense-in-depth: even with the cycle-preserving `seen` map, a very
        // deep *acyclic* chain would recurse to native-stack exhaustion. Cap it
        // and surface a catchable error rather than aborting the host process.
        if depth > crate::value::MAX_RENDER_DEPTH {
            return Err(crate::error::ZapcodeError::RuntimeError(format!(
                "structuredClone nesting depth exceeded (max {})",
                crate::value::MAX_RENDER_DEPTH
            )));
        }
        Ok(match value {
            Value::Array(h) => {
                if let Some(&clone) = seen.get(h) {
                    return Ok(Value::Array(clone));
                }
                // Allocate the clone slot first (empty) and record the mapping so a
                // back-reference to this array resolves to the same clone handle.
                let clone = self.alloc_array(Vec::new());
                seen.insert(*h, clone);
                let items = self.array_vec(*h);
                let mut cloned: Vec<Value> = Vec::with_capacity(items.len());
                for v in items.iter() {
                    cloned.push(self.deep_clone_inner(v, seen, depth + 1)?);
                }
                self.set_array(clone, cloned);
                Value::Array(clone)
            }
            Value::Object(h) => {
                if let Some(&clone) = seen.get(h) {
                    return Ok(Value::Object(clone));
                }
                let clone = self.alloc_object(IndexMap::new());
                seen.insert(*h, clone);
                let map = self.object_map(*h);
                let mut cloned: IndexMap<Arc<str>, Value> = IndexMap::new();
                for (k, v) in map.iter() {
                    cloned.insert(k.clone(), self.deep_clone_inner(v, seen, depth + 1)?);
                }
                self.set_object(clone, cloned);
                Value::Object(clone)
            }
            other => other.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Merging a host-built input heap into a live heap must rebase the input's
    // handles past the slots already present, and keep nested handles valid —
    // this is what the language bindings rely on for array/object inputs and
    // resume values.
    #[test]
    fn absorb_rebases_top_level_and_nested_handles() {
        // Live heap already holds one slot (e.g. a builtin / existing global).
        let mut live = Heap::new();
        let existing = live.alloc_array(vec![Value::Int(1)]);
        assert_eq!(existing, 0);

        // Host builds inputs in a standalone heap: an object { inner: [10, 20] }.
        let mut input = Heap::new();
        let inner = input.alloc_array(vec![Value::Int(10), Value::Int(20)]);
        let mut fields = IndexMap::new();
        fields.insert(Arc::from("inner"), Value::Array(inner));
        let outer = input.alloc_object(fields);
        let input_value = Value::Object(outer);

        let offset = live.absorb(input);
        assert_eq!(offset, 1, "input handle 0 lands at live.len() == 1");

        let rebased = Heap::rebase_handles(input_value, offset);
        let outer_h = match rebased {
            Value::Object(h) => h,
            _ => panic!("expected object"),
        };
        // The object's own handle was rebased, and the live slot it now points
        // at still holds the nested array handle rebased to a valid slot.
        let obj = live.object(outer_h).expect("object present after absorb");
        let nested = match obj.get("inner").unwrap() {
            Value::Array(h) => *h,
            _ => panic!("expected nested array"),
        };
        assert_eq!(live.array(nested), &[Value::Int(10), Value::Int(20)]);
        // The pre-existing live slot is untouched.
        assert_eq!(live.array(existing), &[Value::Int(1)]);
    }

    #[test]
    fn rebase_leaves_primitives_unchanged() {
        assert!(matches!(
            Heap::rebase_handles(Value::Int(5), 7),
            Value::Int(5)
        ));
        assert!(matches!(
            Heap::rebase_handles(Value::Array(2), 3),
            Value::Array(5)
        ));
    }
}

impl Heap {
    /// Re-point every object key at the interner's canonical shared `Arc<str>`.
    /// After a snapshot load each key is a fresh per-occurrence allocation; this
    /// pass (O(total keys)) collapses all occurrences of a string back onto one
    /// backing buffer. Order-preserving: an `IndexMap` rebuilt by draining and
    /// reinserting in iteration order keeps the exact same key sequence, so no
    /// observable behavior changes — only the keys' `Arc` identity.
    pub(crate) fn reintern_keys(&mut self, interner: &mut crate::intern::KeyInterner) {
        for slot in &mut self.slots {
            if let HeapSlot::Object(map) = slot {
                // Drain in order into a fresh map, interning each key. The cost
                // is O(entries) refcount bumps plus at most one string
                // allocation per first-seen key.
                let old = std::mem::take(map);
                let mut rebuilt: IndexMap<Arc<str>, Value> = IndexMap::with_capacity(old.len());
                for (k, v) in old {
                    rebuilt.insert(interner.intern_arc(k), v);
                }
                *map = rebuilt;
            }
        }
    }

    /// A heap holding clones of the first `n` slots (the builtin-template
    /// prefix) — used by the snapshot layer to byte-compare the prefix
    /// against the template before eliding it.
    pub(crate) fn prefix(&self, n: usize) -> Heap {
        Heap {
            slots: self.slots[..n.min(self.slots.len())].to_vec(),
        }
    }

    /// A heap holding clones of the slots from `n` on (everything past the
    /// builtin-template prefix).
    pub(crate) fn tail_from(&self, n: usize) -> Heap {
        Heap {
            slots: self.slots[n.min(self.slots.len())..].to_vec(),
        }
    }

    /// Rebuild a full heap from the builtin template plus a serialized tail
    /// (the inverse of `tail_from` after a template-elided snapshot load).
    pub(crate) fn with_template_prefix(template: &Heap, tail: Heap) -> Heap {
        let mut slots = template.slots.clone();
        slots.extend(tail.slots);
        Heap { slots }
    }
}

impl Heap {
    /// Drop every slot unreachable from `roots` (the builtin-template prefix
    /// `keep_prefix` is always retained, so template elision keeps working),
    /// compacting survivors IN ORDER — the remap is therefore deterministic
    /// and snapshot bytes stay content-addressable. Inner handles of retained
    /// slots are rewritten. Returns the old→new remap (`Handle::MAX` marks a
    /// dropped slot; touching one afterwards is a compactor bug).
    ///
    /// This is the snapshot-time GC: the arena never frees during execution
    /// (handles are stable indices), so a churning agent accumulates dead
    /// slots — without this, every one of them rides in every snapshot
    /// forever.
    pub(crate) fn compact_retaining(
        &mut self,
        keep_prefix: usize,
        roots: &[Handle],
    ) -> Vec<Handle> {
        let n = self.slots.len();
        let mut live = vec![false; n];
        let mut queue: Vec<usize> = Vec::new();
        for i in 0..keep_prefix.min(n) {
            live[i] = true;
            queue.push(i);
        }
        for &h in roots {
            let i = h as usize;
            if i < n && !live[i] {
                live[i] = true;
                queue.push(i);
            }
        }
        let mut children: Vec<Handle> = Vec::new();
        while let Some(i) = queue.pop() {
            children.clear();
            match &mut self.slots[i] {
                HeapSlot::Array(items) => {
                    for v in items.iter_mut() {
                        v.for_each_handle_mut(&mut |h| children.push(*h));
                    }
                }
                HeapSlot::Object(map) => {
                    for (_, v) in map.iter_mut() {
                        v.for_each_handle_mut(&mut |h| children.push(*h));
                    }
                }
            }
            for &c in &children {
                let ci = c as usize;
                if ci < n && !live[ci] {
                    live[ci] = true;
                    queue.push(ci);
                }
            }
        }
        let mut remap = vec![Handle::MAX; n];
        let mut next: Handle = 0;
        for (i, is_live) in live.iter().enumerate() {
            if *is_live {
                remap[i] = next;
                next += 1;
            }
        }
        let mut new_slots = Vec::with_capacity(next as usize);
        for (i, mut slot) in std::mem::take(&mut self.slots).into_iter().enumerate() {
            if !live[i] {
                continue;
            }
            match &mut slot {
                HeapSlot::Array(items) => {
                    for v in items.iter_mut() {
                        v.for_each_handle_mut(&mut |h| *h = remap[*h as usize]);
                    }
                }
                HeapSlot::Object(map) => {
                    for (_, v) in map.iter_mut() {
                        v.for_each_handle_mut(&mut |h| *h = remap[*h as usize]);
                    }
                }
            }
            new_slots.push(slot);
        }
        self.slots = new_slots;
        remap
    }
}
