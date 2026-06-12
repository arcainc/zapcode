use zapcode_core::heap::Heap;
use zapcode_core::vm::{eval_ts, eval_ts_with_output, VmState};
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

/// Run `code` and return the completion value plus the heap that backs any
/// array/object handles in it (needed since `Value::Array`/`Value::Object`
/// only carry a handle into the heap now).
fn run(code: &str) -> (Value, Heap) {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new())
    .unwrap();
    match result.state {
        VmState::Complete(v) => (v, result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

// ── Console ──────────────────────────────────────────────────────────

#[test]
fn test_console_log() {
    let (_, stdout) = eval_ts_with_output("console.log(\"hello\")").unwrap();
    assert_eq!(stdout, "hello\n");
}

#[test]
fn test_console_log_multiple_args() {
    let (_, stdout) = eval_ts_with_output("console.log(1, 2, 3)").unwrap();
    assert_eq!(stdout, "1 2 3\n");
}

#[test]
fn test_console_log_multiline() {
    let (_, stdout) = eval_ts_with_output("console.log(\"a\"); console.log(\"b\")").unwrap();
    assert_eq!(stdout, "a\nb\n");
}

// ── Math ─────────────────────────────────────────────────────────────

#[test]
fn test_math_pi() {
    let result = eval_ts("Math.PI").unwrap();
    match result {
        Value::Float(f) => assert!((f - std::f64::consts::PI).abs() < 1e-10),
        other => panic!("expected float, got {:?}", other),
    }
}

#[test]
fn test_math_floor() {
    let result = eval_ts("Math.floor(4.7)").unwrap();
    assert_eq!(result, Value::Float(4.0));
}

#[test]
fn test_math_ceil() {
    let result = eval_ts("Math.ceil(4.1)").unwrap();
    assert_eq!(result, Value::Float(5.0));
}

#[test]
fn test_math_abs() {
    let result = eval_ts("Math.abs(-42)").unwrap();
    assert_eq!(result, Value::Float(42.0));
}

#[test]
fn test_math_max() {
    let result = eval_ts("Math.max(1, 5, 3)").unwrap();
    assert_eq!(result, Value::Float(5.0));
}

#[test]
fn test_math_min() {
    let result = eval_ts("Math.min(1, 5, 3)").unwrap();
    assert_eq!(result, Value::Float(1.0));
}

#[test]
fn test_math_sqrt() {
    let result = eval_ts("Math.sqrt(16)").unwrap();
    assert_eq!(result, Value::Float(4.0));
}

#[test]
fn test_math_round() {
    let result = eval_ts("Math.round(4.5)").unwrap();
    assert_eq!(result, Value::Float(5.0));
}

#[test]
fn test_math_round_and_sign_negative_zero() {
    // Found by fuzz-differential. Node: Math.round of [-0.5, -0] is -0,
    // Math.sign(-0) is -0, Math.sign(NaN) is NaN. `Object.is` distinguishes
    // -0 from 0, so the sign must survive.
    for code in [
        "Object.is(Math.round(-0.25), -0)",
        "Object.is(Math.round(-0.5), -0)",
        "Object.is(Math.round(-0), -0)",
        "Object.is(Math.sign(-0), -0)",
        "Number.isNaN(Math.sign(NaN))",
        // unchanged positive-side behavior
        "Object.is(Math.round(0.25), 0)",
        "Math.round(-2.5) === -2",
        "Math.sign(-3) === -1",
    ] {
        assert_eq!(eval_ts(code).unwrap(), Value::Bool(true), "{code}");
    }
}

// ── String methods ───────────────────────────────────────────────────

#[test]
fn test_string_to_upper() {
    let result = eval_ts("\"hello\".toUpperCase()").unwrap();
    assert_eq!(result, Value::String("HELLO".into()));
}

#[test]
fn test_string_to_lower() {
    let result = eval_ts("\"HELLO\".toLowerCase()").unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_string_includes() {
    let result = eval_ts("\"hello world\".includes(\"world\")").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_string_starts_with() {
    let result = eval_ts("\"hello\".startsWith(\"hel\")").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_string_ends_with() {
    let result = eval_ts("\"hello\".endsWith(\"llo\")").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_string_index_of() {
    let result = eval_ts("\"hello\".indexOf(\"ll\")").unwrap();
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_string_trim() {
    let result = eval_ts("\"  hello  \".trim()").unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_string_split() {
    let (result, heap) = run("\"a,b,c\".split(\",\")");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::String("a".into()));
            assert_eq!(arr[1], Value::String("b".into()));
            assert_eq!(arr[2], Value::String("c".into()));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_string_replace() {
    let result = eval_ts("\"hello world\".replace(\"world\", \"rust\")").unwrap();
    assert_eq!(result, Value::String("hello rust".into()));
}

#[test]
fn test_string_repeat() {
    let result = eval_ts("\"ab\".repeat(3)").unwrap();
    assert_eq!(result, Value::String("ababab".into()));
}

#[test]
fn test_string_slice() {
    let result = eval_ts("\"hello\".slice(1, 4)").unwrap();
    assert_eq!(result, Value::String("ell".into()));
}

// ── Array methods ────────────────────────────────────────────────────

#[test]
fn test_array_includes() {
    let result = eval_ts("[1, 2, 3].includes(2)").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_array_index_of() {
    let result = eval_ts("[10, 20, 30].indexOf(20)").unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_array_join() {
    let result = eval_ts("[1, 2, 3].join(\"-\")").unwrap();
    assert_eq!(result, Value::String("1-2-3".into()));
}

#[test]
fn test_array_slice() {
    let (result, heap) = run("[1, 2, 3, 4, 5].slice(1, 4)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(2));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_slice_out_of_range_indices_clamp() {
    // Found by fuzz-differential: `[].slice(1, 2)` used to index `arr[1..0]`
    // and panic (aborting the host process via the bindings). Node: all of
    // these clamp into range and yield empty results.
    for (code, expected_len) in [
        ("[].slice(1, 2)", 0),
        ("[1, 2].slice(5)", 0),
        ("[1, 2, 3].slice(2, 1)", 0),
        ("[1, 2, 3].slice(1, 99)", 2),
    ] {
        let (result, heap) = run(code);
        match result {
            Value::Array(h) => assert_eq!(heap.array_vec(h).len(), expected_len, "{code}"),
            other => panic!("expected array for {code}, got {other:?}"),
        }
    }
    let result = eval_ts("\"ab\".slice(5, 9)").unwrap();
    assert_eq!(result, Value::String("".into()));
    let (result, heap) = run("[0, 0].fill(1, 5, 9)");
    match result {
        Value::Array(h) => assert_eq!(heap.array_vec(h), vec![Value::Int(0), Value::Int(0)]),
        other => panic!("expected array, got {other:?}"),
    }
}

#[test]
fn test_array_concat() {
    let (result, heap) = run("[1, 2].concat([3, 4])");
    match result {
        Value::Array(h) => assert_eq!(heap.array_vec(h).len(), 4),
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_reverse() {
    let (result, heap) = run("[1, 2, 3].reverse()");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::Int(3));
            assert_eq!(arr[1], Value::Int(2));
            assert_eq!(arr[2], Value::Int(1));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

// ── JSON ─────────────────────────────────────────────────────────────

#[test]
fn test_json_stringify() {
    let result = eval_ts("JSON.stringify({a: 1, b: 2})").unwrap();
    assert_eq!(result, Value::String("{\"a\":1,\"b\":2}".into()));
}

#[test]
fn test_json_parse() {
    let result = eval_ts("JSON.parse('{\"x\":42}').x").unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── Object static methods ────────────────────────────────────────────

#[test]
fn test_object_keys() {
    let (result, heap) = run("Object.keys({a: 1, b: 2, c: 3})");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::String("a".into()));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_object_values() {
    let (result, heap) = run("Object.values({a: 1, b: 2})");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Int(1));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_is_array() {
    let result = eval_ts("Array.isArray([1, 2])").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_array_is_array_false() {
    let result = eval_ts("Array.isArray(42)").unwrap();
    assert_eq!(result, Value::Bool(false));
}

// ── Array mutating methods ──────────────────────────────────────────

#[test]
fn test_array_push() {
    let result = eval_ts("[1, 2, 3].push(4)").unwrap();
    assert_eq!(result, Value::Int(4));
}

#[test]
fn test_array_push_multiple() {
    let result = eval_ts("[1].push(2, 3, 4)").unwrap();
    assert_eq!(result, Value::Int(4));
}

#[test]
fn test_array_pop() {
    let result = eval_ts("[1, 2, 3].pop()").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_array_pop_empty() {
    let result = eval_ts("[].pop()").unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_array_shift() {
    let result = eval_ts("[1, 2, 3].shift()").unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_array_shift_empty() {
    let result = eval_ts("[].shift()").unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_array_unshift() {
    let result = eval_ts("[3, 4].unshift(1, 2)").unwrap();
    assert_eq!(result, Value::Int(4));
}

#[test]
fn test_array_splice() {
    let (result, heap) = run("[1, 2, 3, 4, 5].splice(1, 2)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Int(2));
            assert_eq!(arr[1], Value::Int(3));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_splice_with_insert() {
    let (result, heap) = run("[1, 2, 3].splice(1, 1, 10, 20)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0], Value::Int(2));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

// ── Array callback methods ──────────────────────────────────────────

#[test]
fn test_array_map() {
    let (result, heap) = run("[1, 2, 3].map((x) => x * 2)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(2));
            assert_eq!(arr[1], Value::Int(4));
            assert_eq!(arr[2], Value::Int(6));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_map_with_index() {
    let (result, heap) = run("[10, 20, 30].map((x, i) => i)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::Int(0));
            assert_eq!(arr[1], Value::Int(1));
            assert_eq!(arr[2], Value::Int(2));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_filter() {
    let (result, heap) = run("[1, 2, 3, 4, 5].filter((x) => x > 3)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Int(4));
            assert_eq!(arr[1], Value::Int(5));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_filter_empty_result() {
    let (result, heap) = run("[1, 2, 3].filter((x) => x > 10)");
    match result {
        Value::Array(h) => assert_eq!(heap.array_vec(h).len(), 0),
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_reduce_with_init() {
    let result = eval_ts("[1, 2, 3, 4].reduce((acc, x) => acc + x, 0)").unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn test_array_reduce_no_init() {
    let result = eval_ts("[1, 2, 3, 4].reduce((acc, x) => acc + x)").unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn test_array_reduce_strings() {
    let result = eval_ts(r#"["a", "b", "c"].reduce((acc, x) => acc + x, "")"#).unwrap();
    assert_eq!(result, Value::String("abc".into()));
}

#[test]
fn test_array_foreach() {
    let (result, stdout) = eval_ts_with_output(
        r#"
        const arr = [1, 2, 3];
        arr.forEach((x) => console.log(x));
        "#,
    )
    .unwrap();
    assert_eq!(result, Value::Undefined);
    assert_eq!(stdout, "1\n2\n3\n");
}

#[test]
fn test_array_find() {
    let result = eval_ts("[1, 2, 3, 4, 5].find((x) => x > 3)").unwrap();
    assert_eq!(result, Value::Int(4));
}

#[test]
fn test_array_find_not_found() {
    let result = eval_ts("[1, 2, 3].find((x) => x > 10)").unwrap();
    assert_eq!(result, Value::Undefined);
}

#[test]
fn test_array_find_index() {
    let result = eval_ts("[1, 2, 3, 4, 5].findIndex((x) => x > 3)").unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_array_find_index_not_found() {
    let result = eval_ts("[1, 2, 3].findIndex((x) => x > 10)").unwrap();
    assert_eq!(result, Value::Int(-1));
}

#[test]
fn test_array_every_true() {
    let result = eval_ts("[2, 4, 6].every((x) => x % 2 === 0)").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_array_every_false() {
    let result = eval_ts("[2, 3, 6].every((x) => x % 2 === 0)").unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_array_some_true() {
    let result = eval_ts("[1, 3, 4].some((x) => x % 2 === 0)").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_array_some_false() {
    let result = eval_ts("[1, 3, 5].some((x) => x % 2 === 0)").unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_array_sort_default() {
    let (result, heap) = run(r#"["banana", "apple", "cherry"].sort()"#);
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::String("apple".into()));
            assert_eq!(arr[1], Value::String("banana".into()));
            assert_eq!(arr[2], Value::String("cherry".into()));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_sort_with_comparator() {
    let (result, heap) = run("[3, 1, 4, 1, 5].sort((a, b) => a - b)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::Int(1));
            assert_eq!(arr[1], Value::Int(1));
            assert_eq!(arr[2], Value::Int(3));
            assert_eq!(arr[3], Value::Int(4));
            assert_eq!(arr[4], Value::Int(5));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_sort_descending() {
    let (result, heap) = run("[3, 1, 4, 1, 5].sort((a, b) => b - a)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::Int(5));
            assert_eq!(arr[1], Value::Int(4));
            assert_eq!(arr[2], Value::Int(3));
            assert_eq!(arr[3], Value::Int(1));
            assert_eq!(arr[4], Value::Int(1));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_flat_map() {
    let (result, heap) = run("[1, 2, 3].flatMap((x) => [x, x * 2])");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 6);
            assert_eq!(arr[0], Value::Int(1));
            assert_eq!(arr[1], Value::Int(2));
            assert_eq!(arr[2], Value::Int(2));
            assert_eq!(arr[3], Value::Int(4));
            assert_eq!(arr[4], Value::Int(3));
            assert_eq!(arr[5], Value::Int(6));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_flat_map_non_array() {
    let (result, heap) = run("[1, 2, 3].flatMap((x) => x * 2)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(2));
            assert_eq!(arr[1], Value::Int(4));
            assert_eq!(arr[2], Value::Int(6));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_map_with_closure() {
    let (result, heap) = run(
        r#"
        const multiplier = 10;
        [1, 2, 3].map((x) => x * multiplier)
        "#,
    );
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr[0], Value::Int(10));
            assert_eq!(arr[1], Value::Int(20));
            assert_eq!(arr[2], Value::Int(30));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_chained_methods() {
    let (result, heap) = run("[1, 2, 3, 4, 5].filter((x) => x % 2 === 0).map((x) => x * 10)");
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Int(20));
            assert_eq!(arr[1], Value::Int(40));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_array_every_empty() {
    // every on empty array returns true (vacuous truth)
    let result = eval_ts("[].every((x) => x > 0)").unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_array_some_empty() {
    // some on empty array returns false
    let result = eval_ts("[].some((x) => x > 0)").unwrap();
    assert_eq!(result, Value::Bool(false));
}
