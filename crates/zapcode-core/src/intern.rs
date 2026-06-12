//! Per-VM object-key interner.
//!
//! Object keys are `Arc<str>` (see [`crate::heap::HeapSlot::Object`]). Without
//! interning, every object-key *insertion* — object literals, `obj.x = …`,
//! computed `obj[k] = …`, spread/rest, builtins that build objects — allocates
//! a FRESH `Arc<str>` even when the same key string ("id", "name", "status", …)
//! has already been allocated thousands of times elsewhere in the heap. An
//! object-heavy agent (e.g. accumulating 200 uniform tool-result records)
//! allocates thousands of key `Arc<str>` for a few dozen unique strings, with
//! ZERO sharing. Each duplicate costs the string bytes plus the ~16-byte `Arc`
//! refcount header (plus allocator slack).
//!
//! The interner is a pool consulted at every key-insertion choke point: a repeat
//! key returns a SHARED `Arc<str>` clone (a refcount bump, no allocation) instead
//! of a new buffer. This collapses the backing allocations down toward one per
//! unique string — reclaiming live RSS and allocator traffic.
//!
//! Crucially this is a pure RUNTIME accelerator and is NEVER serialized: postcard
//! writes each object key's bytes regardless of whether the backing `Arc` is
//! shared, so snapshot bytes are byte-identical with or without interning (the
//! win is live memory, not wire size). Because the pool is rebuilt from scratch,
//! it also imposes no ordering on object maps — `IndexMap` insertion order is
//! untouched, so determinism and every observable behavior are unchanged.

use std::collections::HashMap;
use std::sync::Arc;

/// A per-VM pool mapping a key string to the single shared `Arc<str>` that backs
/// every occurrence of it. Held off the serialized [`crate::vm::Vm`] state (it is
/// simply absent from `VmSnapshot`), so it costs nothing on the wire and is
/// reconstructed cheaply on load via [`KeyInterner::reintern`].
#[derive(Debug, Default)]
pub(crate) struct KeyInterner {
    /// `Box<str>` key (owns its bytes, `Borrow<str>` for `&str` lookups) ->
    /// the canonical `Arc<str>` handed out for that string.
    pool: HashMap<Box<str>, Arc<str>>,
}

impl KeyInterner {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Return the canonical shared `Arc<str>` for `key`, allocating (and
    /// recording) a single backing buffer the first time the string is seen and
    /// cheaply cloning the shared `Arc` (a refcount bump) on every repeat.
    pub(crate) fn intern(&mut self, key: &str) -> Arc<str> {
        if let Some(existing) = self.pool.get(key) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(key);
        self.pool.insert(Box::from(key), arc.clone());
        arc
    }

    /// Fold an already-owned `Arc<str>` into the pool: if the string is already
    /// known, drop the caller's buffer and return the shared one; otherwise adopt
    /// the caller's `Arc` as the canonical entry (no extra allocation). Used by
    /// the post-load pass so a resumed VM regains sharing without re-allocating
    /// strings it just deserialized.
    pub(crate) fn intern_arc(&mut self, key: Arc<str>) -> Arc<str> {
        if let Some(existing) = self.pool.get(key.as_ref()) {
            return existing.clone();
        }
        self.pool.insert(Box::from(key.as_ref()), key.clone());
        key
    }

    /// Re-establish sharing across an entire heap after a snapshot load, where
    /// every key arrived as a fresh per-occurrence `Arc<str>`. Replaces each
    /// object key in place with the pool's canonical `Arc`, so all occurrences of
    /// a string again share one allocation. O(total keys); order-preserving
    /// (only the backing `Arc` identity changes, never the `IndexMap` order).
    pub(crate) fn reintern(&mut self, heap: &mut crate::heap::Heap) {
        heap.reintern_keys(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::Heap;
    use crate::value::Value;
    use indexmap::IndexMap;

    fn addr(a: &Arc<str>) -> usize {
        Arc::as_ptr(a) as *const u8 as usize
    }

    #[test]
    fn repeat_key_returns_one_shared_arc() {
        let mut interner = KeyInterner::new();
        let a = interner.intern("status");
        let b = interner.intern("status");
        // Same backing allocation — a refcount bump, not a new buffer.
        assert_eq!(addr(&a), addr(&b));
        // Distinct strings get distinct allocations.
        let c = interner.intern("id");
        assert_ne!(addr(&a), addr(&c));
    }

    #[test]
    fn intern_arc_adopts_then_dedups() {
        let mut interner = KeyInterner::new();
        let owned: Arc<str> = Arc::from("name");
        let owned_addr = addr(&owned);
        // First sight adopts the caller's buffer as canonical (no realloc).
        let first = interner.intern_arc(owned);
        assert_eq!(addr(&first), owned_addr);
        // A fresh, distinct allocation of the same string folds onto the
        // canonical one, dropping the caller's buffer.
        let dup: Arc<str> = Arc::from("name");
        assert_ne!(addr(&dup), owned_addr, "test setup: dup must be a new buffer");
        let folded = interner.intern_arc(dup);
        assert_eq!(addr(&folded), owned_addr);
    }

    #[test]
    fn reintern_shares_keys_and_preserves_order() {
        // Build two objects with the SAME keys but independent (per-occurrence)
        // `Arc<str>` allocations, exactly as a fresh snapshot load produces.
        let mut heap = Heap::new();
        let mut m1: IndexMap<Arc<str>, Value> = IndexMap::new();
        m1.insert(Arc::from("id"), Value::Int(1));
        m1.insert(Arc::from("name"), Value::Int(2));
        let h1 = heap.alloc_object(m1);
        let mut m2: IndexMap<Arc<str>, Value> = IndexMap::new();
        m2.insert(Arc::from("id"), Value::Int(3));
        m2.insert(Arc::from("name"), Value::Int(4));
        let h2 = heap.alloc_object(m2);

        // Pre-pass: the two objects' "id" keys are DISTINCT allocations.
        let id1_before = addr(heap.object(h1).unwrap().keys().next().unwrap());
        let id2_before = addr(heap.object(h2).unwrap().keys().next().unwrap());
        assert_ne!(id1_before, id2_before);

        let mut interner = KeyInterner::new();
        interner.reintern(&mut heap);

        let o1 = heap.object(h1).unwrap();
        let o2 = heap.object(h2).unwrap();
        // Order preserved.
        assert_eq!(o1.keys().map(|k| k.to_string()).collect::<Vec<_>>(), ["id", "name"]);
        assert_eq!(o2.keys().map(|k| k.to_string()).collect::<Vec<_>>(), ["id", "name"]);
        // Same string now shares ONE allocation across both objects.
        let id1 = addr(o1.keys().next().unwrap());
        let id2 = addr(o2.keys().next().unwrap());
        assert_eq!(id1, id2, "reintern must collapse duplicate key allocations");
        let name1 = addr(o1.keys().nth(1).unwrap());
        let name2 = addr(o2.keys().nth(1).unwrap());
        assert_eq!(name1, name2);
        // Values untouched.
        assert_eq!(o1.get("id"), Some(&Value::Int(1)));
        assert_eq!(o2.get("name"), Some(&Value::Int(4)));
    }
}
