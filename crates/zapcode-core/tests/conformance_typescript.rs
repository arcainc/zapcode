//! Conformance breadth: the TypeScript-subset surface — type-syntax STRIPPING.
//!
//! zapcode is a TypeScript-subset interpreter: type annotations, interfaces, and
//! type aliases are parsed and *erased* (they carry no runtime meaning, exactly
//! like `tsc`/`esbuild` transpile-only). This suite pins that erasure across the
//! breadth of TS type syntax an agent (or an LLM emitting TS) is likely to write,
//! asserting that the surrounding runtime program behaves identically to the
//! type-free version. Every asserted value was cross-checked against real `node`
//! running the type-stripped equivalent.
//!
//! FULLY-STRIPPED (program runs as if the types weren't there):
//!   - variable / parameter / return-type annotations (`const x: number`,
//!     `(a: T): R =>`), incl. optional params (`x?: T`) and defaults with a type;
//!   - `interface` (incl. `extends`) and `type` aliases (unions, tuples, function
//!     types, index signatures, generic / conditional / intersection / `keyof` /
//!     `typeof` / template-literal types);
//!   - generics: `function f<T>()`, `class Box<T>`, explicit call/`new`/method type
//!     arguments (`id<string>(x)`, `new Map<K, V>()`);
//!   - `as` casts (incl. chained `as unknown as T`), `satisfies`, definite
//!     assignment (`let s!: T`), `readonly`, `declare`, `this`-parameters;
//!   - decorators are accepted and ignored.
//!
//! DOCUMENTED UNSUPPORTED (asserted at zapcode's ACTUAL error, never the JS/TS
//! runtime answer):
//!   - `enum` declarations (TS enums emit runtime code; explicitly rejected);
//!   - `namespace` / `module` blocks (rejected as an unsupported statement);
//!   - the legacy angle-bracket cast `<T>expr` (a parse error — conflicts with the
//!     JSX/`tsx` grammar the parser uses; use the `expr as T` form instead).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
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
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

fn run_err(code: &str) -> String {
    let result = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        Vec::new(),
        ResourceLimits::default(),
    )
    .unwrap()
    .run(Vec::new());
    match result {
        Ok(_) => panic!("expected an error for `{code}`, but it completed"),
        Err(e) => format!("{e:?}"),
    }
}

// ============================================================================
// Variable annotations
// ============================================================================

#[test]
fn const_let_annotations_are_stripped() {
    assert_eq!(run_str("const x: number = 5; x + 1"), "6");
    assert_eq!(run_str("let y: string = 'hi'; y.length"), "2");
    assert_eq!(run_str("const b: boolean = true; b ? 'y' : 'n'"), "y");
    assert_eq!(run_str("const n: null = null; String(n)"), "null");
}

#[test]
fn uninitialized_annotated_let_then_assign() {
    assert_eq!(run_str("let v: number; v = 3; v"), "3");
    assert_eq!(run_str("let s: string; s = 'x'; s"), "x");
}

#[test]
fn definite_assignment_assertion() {
    // `let s!: string` — definite-assignment operator stripped.
    assert_eq!(run_str("let s!: string; s = 'x'; s"), "x");
}

#[test]
fn array_and_collection_annotations() {
    assert_eq!(run_str("const arr: number[] = [1, 2, 3]; arr.length"), "3");
    assert_eq!(run_str("const arr: Array<number> = [1, 2]; arr[0] + arr[1]"), "3");
    assert_eq!(run_str("const tup: readonly number[] = [1, 2, 3]; tup.length"), "3");
}

// ============================================================================
// Function & arrow annotations
// ============================================================================

#[test]
fn function_param_and_return_annotations() {
    assert_eq!(
        run_str("function f(a: number, b: string): string { return b.repeat(a); } f(3, 'ab')"),
        "ababab"
    );
    assert_eq!(run_str("function f(): void {} String(f())"), "undefined");
}

#[test]
fn arrow_annotations() {
    assert_eq!(run_str("const g = (n: number): number => n * n; g(4)"), "16");
    assert_eq!(
        run_str("const f = (a: number, b: number): number => a + b; f(10, 20)"),
        "30"
    );
}

#[test]
fn named_function_expression_with_annotations() {
    assert_eq!(
        run_str("const fn = function named(n: number): number { return n; }; fn(8)"),
        "8"
    );
}

#[test]
fn optional_parameter() {
    assert_eq!(run_str("function f(x?: number) { return x ?? -1; } String(f())"), "-1");
    assert_eq!(run_str("function f(x?: number) { return x ?? -1; } f(7)"), "7");
}

#[test]
fn default_parameter_with_annotation() {
    assert_eq!(run_str("function f(a: number = 10) { return a; } f()"), "10");
    assert_eq!(run_str("function f(a: number = 10) { return a; } f(3)"), "3");
}

#[test]
fn rest_parameter_with_annotation() {
    assert_eq!(
        run_str("function f(...args: number[]): number { return args.length; } f(1, 2, 3)"),
        "3"
    );
}

#[test]
fn this_parameter_is_stripped() {
    // A leading `this: T` parameter is a TS type-only construct, not a real arg.
    assert_eq!(run_str("function f(this: void, x: number) { return x; } f(7)"), "7");
}

// ============================================================================
// Interfaces & type aliases (purely erased)
// ============================================================================

#[test]
fn interface_declaration_is_erased() {
    assert_eq!(
        run_str("interface Shape { kind: string; size: number } const s: Shape = { kind: 'sq', size: 4 }; s.kind"),
        "sq"
    );
}

#[test]
fn interface_extends_is_erased() {
    assert_eq!(
        run_str("interface A { x: number } interface B extends A { y: number } const b: B = { x: 1, y: 2 }; b.x + b.y"),
        "3"
    );
}

#[test]
fn interface_with_method_signature_implemented_by_class() {
    assert_eq!(
        run_str("interface Animal { sound(): string } class Dog implements Animal { sound() { return 'woof'; } } new Dog().sound()"),
        "woof"
    );
}

#[test]
fn type_alias_union_and_literal() {
    assert_eq!(run_str("type ID = string | number; const i: ID = 7; i"), "7");
    assert_eq!(run_str("type Status = 'on' | 'off'; const st: Status = 'on'; st"), "on");
}

#[test]
fn type_alias_tuple() {
    assert_eq!(run_str("type Pair = [number, string]; const p: Pair = [1, 'a']; p[1]"), "a");
}

#[test]
fn type_alias_function_type() {
    assert_eq!(
        run_str("type Fn = (a: number) => number; const f: Fn = a => a + 1; f(5)"),
        "6"
    );
}

#[test]
fn type_alias_index_signature() {
    assert_eq!(
        run_str("const o: { [key: string]: number } = { a: 1, b: 2 }; o.a + o.b"),
        "3"
    );
}

#[test]
fn type_alias_record_utility() {
    assert_eq!(
        run_str("type Rec = Record<string, number>; const r: Rec = { a: 1 }; r.a"),
        "1"
    );
}

#[test]
fn type_alias_generic_conditional_intersection_keyof() {
    assert_eq!(run_str("type Maybe<T> = T | null; const x: Maybe<number> = 5; x"), "5");
    assert_eq!(
        run_str("type Cond<X> = X extends string ? 1 : 2; const x: Cond<string> = 1; x"),
        "1"
    );
    assert_eq!(run_str("const x: number & {} = 5; x"), "5");
    assert_eq!(run_str("type K = keyof { a: number; b: number }; const k: K = 'a'; k"), "a");
}

#[test]
fn type_alias_template_literal_type() {
    assert_eq!(
        run_str("type T = `prefix-${string}`; const a: T = 'prefix-x'; a"),
        "prefix-x"
    );
}

// ============================================================================
// Generics at definition & call sites
// ============================================================================

#[test]
fn generic_function_definition() {
    assert_eq!(run_str("function id<T>(x: T): T { return x; } id(42)"), "42");
    assert_eq!(
        run_str("function wrap<T>(v: T): T[] { return [v]; } wrap(9).length"),
        "1"
    );
}

#[test]
fn generic_class_definition() {
    assert_eq!(
        run_str("class Box<T> { v: T; constructor(v: T) { this.v = v; } get() { return this.v; } } new Box(7).get()"),
        "7"
    );
}

#[test]
fn explicit_call_type_arguments() {
    assert_eq!(
        run_str("function id<T>(v: T): T { return v; } id<string>('x')"),
        "x"
    );
    assert_eq!(
        run_str("function id<T>(v: T): T { return v; } const a = id<number>(5); a"),
        "5"
    );
}

#[test]
fn new_with_type_arguments() {
    assert_eq!(
        run_str("const m = new Map<string, number>(); m.set('a', 1); m.get('a')"),
        "1"
    );
    assert_eq!(
        run_str("const m: Map<string, number[]> = new Map(); m.set('a', [1, 2]); m.get('a').length"),
        "2"
    );
}

#[test]
fn method_call_type_arguments() {
    assert_eq!(run_str("Array.from<number>([1, 2, 3]).length"), "3");
}

// ============================================================================
// Casts, satisfies
// ============================================================================

#[test]
fn as_cast_is_stripped() {
    assert_eq!(run_str("let v = 10 as number; v"), "10");
    assert_eq!(run_str("const s = 'hi' as string; s.length"), "2");
}

#[test]
fn chained_as_cast() {
    assert_eq!(run_str("const n = (5 as unknown) as number; n"), "5");
    assert_eq!(run_str("let u: unknown = 'hi'; (u as string).length"), "2");
}

#[test]
fn as_const_is_stripped() {
    assert_eq!(run_str("const o = { a: 1 } as const; o.a"), "1");
    assert_eq!(run_str("const t = [1, 2, 3] as const; t.length"), "3");
}

#[test]
fn satisfies_is_stripped() {
    assert_eq!(run_str("const x = 5 satisfies number; x"), "5");
    assert_eq!(
        run_str("const obj = { a: 1 } satisfies { a: number }; obj.a"),
        "1"
    );
}

// ============================================================================
// Class-level TS constructs
// ============================================================================

#[test]
fn class_method_with_annotations() {
    assert_eq!(
        run_str("class C { add(a: number, b: number): number { return a + b; } } new C().add(2, 3)"),
        "5"
    );
}

#[test]
fn abstract_class_is_accepted() {
    // `abstract` is a TS-only modifier; the declaration is accepted.
    assert_eq!(run_str("abstract class A { } 'ok'"), "ok");
}

#[test]
fn decorator_is_accepted_and_ignored() {
    // A class decorator is parsed and ignored (no decorator runtime).
    assert_eq!(run_str("@dec class C {} 'ok'"), "ok");
}

// ============================================================================
// Ambient / type-only forms
// ============================================================================

#[test]
fn declare_statements_are_erased() {
    assert_eq!(run_str("declare const g: number; 'ok'"), "ok");
    assert_eq!(run_str("declare function ext(x: number): number; 'skipped'"), "skipped");
}

// ============================================================================
// Type-stripping must not change runtime semantics
// ============================================================================

#[test]
fn annotated_and_unannotated_programs_agree() {
    // The same computation, with and without type syntax, yields the same result.
    let typed =
        "function sum(xs: number[]): number { let t: number = 0; for (const x of xs) t += x; return t; } sum([1, 2, 3, 4])";
    let untyped =
        "function sum(xs) { let t = 0; for (const x of xs) t += x; return t; } sum([1, 2, 3, 4])";
    assert_eq!(run_str(typed), run_str(untyped));
    assert_eq!(run_str(typed), "10");
}

#[test]
fn types_do_not_constrain_runtime_values() {
    // Annotations are erased, so a "number"-typed binding can hold any value at
    // runtime — the type is not enforced (transpile-only semantics, like tsc).
    assert_eq!(run_str("let n: number = 5; n = 'now a string'; typeof n"), "string");
}

// ============================================================================
// DOCUMENTED UNSUPPORTED forms
// ============================================================================

#[test]
fn enum_is_unsupported() {
    // DIVERGENCE asserted as actual: TS `enum` emits runtime code and is rejected.
    let e = run_err("enum Color { Red, Green, Blue } Color.Green");
    assert!(e.contains("enum") || e.contains("unsupported"), "got: {e}");
}

#[test]
fn namespace_is_unsupported() {
    // DIVERGENCE asserted as actual: a `namespace` block is rejected.
    let e = run_err("namespace N { export const v = 1 } 'x'");
    assert!(e.contains("unsupported"), "got: {e}");
}

#[test]
fn module_block_is_unsupported() {
    // DIVERGENCE asserted as actual: a `module M {}` block is rejected.
    let e = run_err("module M { export const v = 1 } 'x'");
    assert!(e.contains("unsupported"), "got: {e}");
}

#[test]
fn legacy_angle_bracket_cast_is_a_parse_error() {
    // DIVERGENCE asserted as actual: the legacy `<T>expr` cast conflicts with the
    // tsx/JSX grammar and is a parse error; the `expr as T` form is the supported
    // way to assert a type.
    let e = run_err("const x = <number>5; x");
    assert!(e.to_lowercase().contains("parse") || e.contains("Unexpected"), "got: {e}");
}
