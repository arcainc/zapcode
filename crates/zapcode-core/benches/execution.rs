use zapcode_core::vm::eval_ts;

fn main() {
    divan::main();
}

#[divan::bench]
fn simple_expression() -> zapcode_core::Value {
    eval_ts("1 + 2 * 3").unwrap()
}

#[divan::bench]
fn variable_arithmetic() -> zapcode_core::Value {
    eval_ts("const x = 42; const y = 58; x + y").unwrap()
}

#[divan::bench]
fn function_call() -> zapcode_core::Value {
    eval_ts("function add(a, b) { return a + b; } add(1, 2)").unwrap()
}

#[divan::bench]
fn loop_100() -> zapcode_core::Value {
    eval_ts("let sum = 0; for (let i = 0; i < 100; i++) { sum += i; } sum").unwrap()
}

#[divan::bench]
fn fibonacci_10() -> zapcode_core::Value {
    eval_ts("function fib(n) { if (n <= 1) { return n; } return fib(n - 1) + fib(n - 2); } fib(10)")
        .unwrap()
}

#[divan::bench]
fn object_creation() -> zapcode_core::Value {
    eval_ts("const obj = { a: 1, b: 2, c: 3 }; obj.a + obj.b + obj.c").unwrap()
}

#[divan::bench]
fn array_creation() -> zapcode_core::Value {
    eval_ts("[1, 2, 3, 4, 5]").unwrap()
}

#[divan::bench]
fn string_concat() -> zapcode_core::Value {
    eval_ts("\"hello\" + \" \" + \"world\"").unwrap()
}

#[divan::bench]
fn template_literal() -> zapcode_core::Value {
    eval_ts("const name = \"world\"; `hello ${name}`").unwrap()
}

#[divan::bench]
fn promise_resolve_await() -> zapcode_core::Value {
    eval_ts("await Promise.resolve(42)").unwrap()
}

#[divan::bench]
fn promise_then_single() -> zapcode_core::Value {
    eval_ts("await Promise.resolve(10).then(x => x * 2)").unwrap()
}

#[divan::bench]
fn promise_then_chain_3() -> zapcode_core::Value {
    eval_ts("await Promise.resolve(1).then(x => x + 1).then(x => x * 2).then(x => x + 10)").unwrap()
}

#[divan::bench]
fn promise_catch_resolved() -> zapcode_core::Value {
    eval_ts("await Promise.resolve(42).catch(e => 0)").unwrap()
}

#[divan::bench]
fn promise_all_3() -> zapcode_core::Value {
    eval_ts("await Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)])")
        .unwrap()
}

#[divan::bench]
fn async_map_3() -> zapcode_core::Value {
    eval_ts(
        r#"
        const items = [1, 2, 3];
        items.map(async (x) => {
            const doubled = await Promise.resolve(x * 2);
            return doubled;
        })
    "#,
    )
    .unwrap()
}
