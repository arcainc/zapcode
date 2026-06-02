use zapcode_core::heap::Heap;
use zapcode_core::vm::{eval_ts, eval_ts_with_output, VmState};
use zapcode_core::{ResourceLimits, Value, ZapcodeRun};

/// Run `code` and return the completion value plus the heap that backs any
/// array/object handles in it.
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

#[test]
fn test_fizzbuzz() {
    let (_, stdout) = eval_ts_with_output(
        r#"
        for (let i = 1; i <= 15; i++) {
            if (i % 15 === 0) {
                console.log("FizzBuzz");
            } else if (i % 3 === 0) {
                console.log("Fizz");
            } else if (i % 5 === 0) {
                console.log("Buzz");
            } else {
                console.log(i);
            }
        }
    "#,
    )
    .unwrap();
    let lines: Vec<&str> = stdout.trim().split('\n').collect();
    assert_eq!(lines.len(), 15);
    assert_eq!(lines[0], "1");
    assert_eq!(lines[2], "Fizz");
    assert_eq!(lines[4], "Buzz");
    assert_eq!(lines[14], "FizzBuzz");
}

#[test]
fn test_map_implementation() {
    let (result, heap) = run(
        r#"
        function map(arr, fn) {
            const result = [];
            for (let i = 0; i < arr.length; i++) {
                result[i] = fn(arr[i], i);
            }
            return result;
        }
        const doubled = map([1, 2, 3], (x) => x * 2);
        doubled
    "#,
    );
    match result {
        Value::Array(h) => {
            assert_eq!(
                heap.array_vec(h),
                vec![Value::Int(2), Value::Int(4), Value::Int(6)]
            );
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_reduce_implementation() {
    let result = eval_ts(
        r#"
        function reduce(arr, fn, init) {
            let acc = init;
            for (let i = 0; i < arr.length; i++) {
                acc = fn(acc, arr[i]);
            }
            return acc;
        }
        reduce([1, 2, 3, 4, 5], (sum, x) => sum + x, 0)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(15));
}

#[test]
fn test_closure() {
    let result = eval_ts(
        r#"
        function makeCounter() {
            let count = 0;
            return () => {
                count = count + 1;
                return count;
            };
        }
        const counter = makeCounter();
        counter();
        counter();
        counter()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_typescript_types_stripped() {
    let result = eval_ts(
        r#"
        const x: number = 42;
        const y: string = "hello";
        interface Foo {
            bar: number;
        }
        type Result = string | number;
        x
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_optional_chaining() {
    let result = eval_ts(
        r#"
        const obj = { a: { b: 42 } };
        const x = obj?.a?.b;
        const y = obj?.c?.d;
        x
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_json_round_trip() {
    let result = eval_ts(
        r#"
        const obj = { name: "zapcode", version: 1 };
        const json = JSON.stringify(obj);
        const parsed = JSON.parse(json);
        parsed.name + " v" + parsed.version
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("zapcode v1".into()));
}

#[test]
fn test_string_processing() {
    let result = eval_ts(
        r#"
        const words = "hello world foo bar".split(" ");
        const upper = [];
        for (let i = 0; i < words.length; i++) {
            upper[i] = words[i].toUpperCase();
        }
        upper.join(", ")
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("HELLO, WORLD, FOO, BAR".into()));
}

#[test]
fn test_nested_functions() {
    let result = eval_ts(
        r#"
        function compose(f, g) {
            return (x) => f(g(x));
        }
        const double = (x) => x * 2;
        const inc = (x) => x + 1;
        const doubleInc = compose(double, inc);
        doubleInc(20)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_error_recovery() {
    let result = eval_ts(
        r#"
        let result = "default";
        try {
            const obj = null;
            result = obj.foo;
        } catch (e) {
            result = "caught";
        }
        result
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("caught".into()));
}

#[test]
fn test_complex_data_processing() {
    let result = eval_ts(
        r#"
        const data = [
            { name: "Alice", score: 90 },
            { name: "Bob", score: 85 },
            { name: "Charlie", score: 95 },
        ];

        let highest = data[0];
        for (let i = 1; i < data.length; i++) {
            if (data[i].score > highest.score) {
                highest = data[i];
            }
        }
        highest.name
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Charlie".into()));
}

#[test]
fn test_math_operations() {
    let result = eval_ts(
        r#"
        const hypotenuse = Math.sqrt(Math.pow(3, 2) + Math.pow(4, 2));
        Math.round(hypotenuse)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Float(5.0));
}
