use zapcode_core::heap::Heap;
use zapcode_core::vm::{eval_ts, VmState};
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
fn test_basic_class_with_constructor() {
    let result = eval_ts(
        r#"
        class Animal {
            name: string;
            constructor(name: string) {
                this.name = name;
            }
        }
        const a = new Animal("Dog");
        a.name
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Dog".into()));
}

#[test]
fn test_class_with_method() {
    let result = eval_ts(
        r#"
        class Animal {
            name: string;
            constructor(name: string) {
                this.name = name;
            }
            speak() {
                return this.name + " makes a sound";
            }
        }
        const a = new Animal("Dog");
        a.speak()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Dog makes a sound".into()));
}

#[test]
fn test_class_multiple_properties() {
    let result = eval_ts(
        r#"
        class Point {
            x: number;
            y: number;
            constructor(x: number, y: number) {
                this.x = x;
                this.y = y;
            }
            sum() {
                return this.x + this.y;
            }
        }
        const p = new Point(3, 4);
        p.sum()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(7));
}

#[test]
fn test_class_inheritance() {
    let result = eval_ts(
        r#"
        class Animal {
            name: string;
            constructor(name: string) {
                this.name = name;
            }
            speak() {
                return this.name + " makes a sound";
            }
        }
        class Dog extends Animal {
            constructor(name: string) {
                super(name);
            }
            speak() {
                return this.name + " barks";
            }
        }
        const d = new Dog("Rex");
        d.speak()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Rex barks".into()));
}

#[test]
fn test_class_inheritance_parent_method() {
    let result = eval_ts(
        r#"
        class Animal {
            name: string;
            constructor(name: string) {
                this.name = name;
            }
            speak() {
                return this.name + " makes a sound";
            }
            getName() {
                return this.name;
            }
        }
        class Dog extends Animal {
            constructor(name: string) {
                super(name);
            }
            speak() {
                return this.name + " barks";
            }
        }
        const d = new Dog("Rex");
        d.getName()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Rex".into()));
}

#[test]
fn test_static_method() {
    let result = eval_ts(
        r#"
        class MathUtil {
            static add(a: number, b: number) {
                return a + b;
            }
        }
        MathUtil.add(1, 2)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_static_method_multiple() {
    let result = eval_ts(
        r#"
        class MathUtil {
            static add(a: number, b: number) {
                return a + b;
            }
            static mul(a: number, b: number) {
                return a * b;
            }
        }
        MathUtil.add(2, 3) + MathUtil.mul(4, 5)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(25));
}

#[test]
fn test_class_no_constructor() {
    let result = eval_ts(
        r#"
        class Greeter {
            greet() {
                return "hello";
            }
        }
        const g = new Greeter();
        g.greet()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("hello".into()));
}

#[test]
fn test_class_method_chaining() {
    // Method chaining works by using the return value. Mutations to `this`
    // inside methods are also written back to the receiver variable.
    let result = eval_ts(
        r#"
        class Builder {
            value: number;
            constructor() {
                this.value = 0;
            }
            add(n: number) {
                this.value = this.value + n;
                return this;
            }
            getResult() {
                return this.value;
            }
        }
        const b = new Builder();
        b.add(5).add(3).getResult()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(8));
}

#[test]
fn test_instanceof() {
    let result = eval_ts(
        r#"
        class Animal {
            constructor() {}
        }
        const a = new Animal();
        a instanceof Animal
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_class_property_access() {
    let result = eval_ts(
        r#"
        class Config {
            host: string;
            port: number;
            constructor(host: string, port: number) {
                this.host = host;
                this.port = port;
            }
        }
        const c = new Config("localhost", 8080);
        c.host + ":" + c.port
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("localhost:8080".into()));
}

#[test]
fn test_class_expression() {
    let result = eval_ts(
        r#"
        const MyClass = class {
            getValue() {
                return 42;
            }
        };
        const obj = new MyClass();
        obj.getValue()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_class_with_multiple_methods() {
    let result = eval_ts(
        r#"
        class Calculator {
            value: number;
            constructor(initial: number) {
                this.value = initial;
            }
            add(n: number) {
                return this.value + n;
            }
            multiply(n: number) {
                return this.value * n;
            }
        }
        const calc = new Calculator(10);
        calc.add(5) + calc.multiply(3)
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(45));
}

#[test]
fn test_multiple_instances() {
    let result = eval_ts(
        r#"
        class Counter {
            count: number;
            constructor(start: number) {
                this.count = start;
            }
            getCount() {
                return this.count;
            }
        }
        const a = new Counter(10);
        const b = new Counter(20);
        a.getCount() + b.getCount()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(30));
}

#[test]
fn test_method_this_mutation_persists() {
    // Regression test: mutations to `this` properties inside method calls must
    // persist back to the original object variable. Previously, the receiver was
    // cloned and mutations were lost.
    let (result, heap) = run(
        r#"
        class Counter {
            count: number;
            constructor(start: number) {
                this.count = start;
            }
            increment() {
                this.count += 1;
                return this.count;
            }
        }
        const c = new Counter(10);
        [c.increment(), c.increment(), c.increment()]
    "#,
    );
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], Value::Int(11));
            assert_eq!(arr[1], Value::Int(12));
            assert_eq!(arr[2], Value::Int(13));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_method_this_mutation_persists_local() {
    // Same as above but ensuring mutations work in a function scope (local variables).
    let result = eval_ts(
        r#"
        class Counter {
            count: number;
            constructor(start: number) {
                this.count = start;
            }
            increment() {
                this.count += 1;
                return this.count;
            }
        }
        function test() {
            const c = new Counter(0);
            c.increment();
            c.increment();
            return c.count;
        }
        test()
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_method_this_mutation_multiple_instances() {
    // Ensure mutations to one instance don't affect another.
    let (result, heap) = run(
        r#"
        class Counter {
            count: number;
            constructor(start: number) {
                this.count = start;
            }
            increment() {
                this.count += 1;
                return this.count;
            }
        }
        const a = new Counter(0);
        const b = new Counter(100);
        a.increment();
        a.increment();
        b.increment();
        [a.count, b.count]
    "#,
    );
    match result {
        Value::Array(h) => {
            let arr = heap.array_vec(h);
            assert_eq!(arr.len(), 2);
            assert_eq!(arr[0], Value::Int(2));
            assert_eq!(arr[1], Value::Int(101));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

#[test]
fn test_class_with_string_method() {
    let result = eval_ts(
        r#"
        class Greeter {
            prefix: string;
            constructor(prefix: string) {
                this.prefix = prefix;
            }
            greet(name: string) {
                return this.prefix + " " + name;
            }
        }
        const g = new Greeter("Hello");
        g.greet("World")
    "#,
    )
    .unwrap();
    assert_eq!(result, Value::String("Hello World".into()));
}
