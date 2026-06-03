//! Integration coverage for the heap-seeding input path used by the language
//! bindings: array/object inputs are allocated into a standalone heap and must
//! resolve correctly once the VM appends its builtins on top.

use indexmap::IndexMap;
use std::sync::Arc;
use zapcode_core::heap::Heap;
use zapcode_core::{ResourceLimits, Value, VmState, ZapcodeRun};

fn run_with_input(source: &str, name: &str, value: Value, heap: Heap) -> (Value, Heap) {
    let runner = ZapcodeRun::new(
        source.to_string(),
        vec![name.to_string()],
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap();
    let result = runner
        .run_with_input_heap(vec![(name.to_string(), value)], heap)
        .unwrap();
    match result.state {
        VmState::Complete(v) => (v, result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn array_input_handle_resolves_after_builtins_appended() {
    // Host builds the input array in a fresh heap (handle 0). The VM appends its
    // builtin slots on top, so the input handle must stay valid.
    let mut heap = Heap::new();
    let arr = heap.alloc_array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let (out, out_heap) = run_with_input(
        "items.reduce((a, b) => a + b, 0)",
        "items",
        Value::Array(arr),
        heap,
    );
    assert_eq!(out, Value::Int(6), "array input summed correctly");
    let _ = out_heap;
}

#[test]
fn object_input_round_trips_and_can_be_mutated() {
    let mut heap = Heap::new();
    let mut fields = IndexMap::new();
    fields.insert(Arc::from("count"), Value::Int(41));
    let obj = heap.alloc_object(fields);

    let (out, out_heap) = run_with_input(
        "cfg.count = cfg.count + 1; cfg",
        "cfg",
        Value::Object(obj),
        heap,
    );

    let h = match out {
        Value::Object(h) => h,
        other => panic!("expected object output, got {other:?}"),
    };
    let map = out_heap.object(h).expect("object present in result heap");
    assert_eq!(map.get("count"), Some(&Value::Int(42)));
}

#[test]
fn nested_array_of_objects_input_resolves() {
    let mut heap = Heap::new();
    let mut a = IndexMap::new();
    a.insert(Arc::from("v"), Value::Int(10));
    let oa = heap.alloc_object(a);
    let mut b = IndexMap::new();
    b.insert(Arc::from("v"), Value::Int(20));
    let ob = heap.alloc_object(b);
    let arr = heap.alloc_array(vec![Value::Object(oa), Value::Object(ob)]);

    let (out, _) = run_with_input(
        "rows.map(r => r.v).reduce((a, b) => a + b, 0)",
        "rows",
        Value::Array(arr),
        heap,
    );
    assert_eq!(out, Value::Int(30));
}
