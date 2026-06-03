//! Conformance breadth: classes & inheritance.
//!
//! Covers the fully-supported class surface — constructors, instance methods,
//! `extends`/`super(...)`/`super.method()` (multi-level), implicit constructors,
//! method overriding, `instanceof` ancestry, static methods, `this`-chaining,
//! `toString` coercion hooks, and class expressions. The deferred "cluster C" gaps
//! (class FIELD declarations, STATIC fields, GETTERS/SETTERS, and `#private`
//! fields) are pinned to zapcode's actual behavior with comments — instance state
//! goes through the constructor here, which is the supported idiom.

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

// ----------------------------------------------------------------------------
// Construction & methods
// ----------------------------------------------------------------------------

#[test]
fn basic_class_and_methods() {
    assert_eq!(
        run_str("class P { constructor(n){ this.n = n; } hi(){ return 'hi ' + this.n; } } new P('a').hi()"),
        "hi a"
    );
    assert_eq!(
        run_str("class Point { constructor(x, y){ this.x = x; this.y = y; } sum(){ return this.x + this.y; } } new Point(3, 4).sum()"),
        "7"
    );
}

#[test]
fn instance_state_via_constructor() {
    // The supported idiom for instance fields is constructor assignment.
    assert_eq!(
        run_str("class C { constructor(){ this.x = 10; this.y = this.x * 2; } } const c = new C(); `${c.x},${c.y}`"),
        "10,20"
    );
}

#[test]
fn this_chaining_methods() {
    assert_eq!(
        run_str("class Counter { constructor(){ this.n = 0; } inc(){ this.n++; return this; } } const c = new Counter(); c.inc().inc().inc().n"),
        "3"
    );
}

#[test]
fn to_string_hook_on_instance() {
    assert_eq!(
        run_str("class P { constructor(n){ this.n = n; } toString(){ return 'P(' + this.n + ')'; } } '' + new P(3)"),
        "P(3)"
    );
}

#[test]
fn class_expression() {
    assert_eq!(run_str("const C = class { f(){ return 7; } }; new C().f()"), "7");
    assert_eq!(run_str("const Named = class Inner { val(){ return 11; } }; new Named().val()"), "11");
}

// ----------------------------------------------------------------------------
// Inheritance & super
// ----------------------------------------------------------------------------

#[test]
fn inheritance_inherits_methods() {
    assert_eq!(run_str("class A { f(){ return 1; } } class B extends A {} new B().f()"), "1");
    assert_eq!(
        run_str("class Animal { speak(){ return 'generic'; } } class Dog extends Animal {} new Dog().speak()"),
        "generic"
    );
}

#[test]
fn super_constructor() {
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A { constructor(){ super(5); } } new B().x"),
        "5"
    );
    // implicit constructor forwards args to super
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A {} new B(7).x"),
        "7"
    );
}

#[test]
fn super_constructor_then_own_fields() {
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A { constructor(x){ super(x); this.doubled = x * 2; } } const b = new B(4); `${b.x},${b.doubled}`"),
        "4,8"
    );
}

#[test]
fn super_method_call() {
    assert_eq!(
        run_str("class A { g(){ return 10; } } class B extends A { g(){ return super.g() + 5; } } new B().g()"),
        "15"
    );
}

#[test]
fn super_method_multi_level() {
    assert_eq!(
        run_str("class A { g(){ return 1; } } class B extends A { g(){ return super.g() + 10; } } class C extends B { g(){ return super.g() + 100; } } new C().g()"),
        "111"
    );
}

#[test]
fn super_reads_this_set_after_super_call() {
    assert_eq!(
        run_str("class A { describe(){ return 'A:' + this.tag; } } class B extends A { constructor(){ super(); this.tag = 'b'; } describe(){ return super.describe() + '|B'; } } new B().describe()"),
        "A:b|B"
    );
}

#[test]
fn method_overriding() {
    assert_eq!(
        run_str("class A { name(){ return 'A'; } } class B extends A { name(){ return 'B'; } } new B().name()"),
        "B"
    );
    assert_eq!(
        run_str("class Shape { area(){ return 0; } } class Square extends Shape { constructor(s){ super(); this.s = s; } area(){ return this.s * this.s; } } new Square(5).area()"),
        "25"
    );
}

// ----------------------------------------------------------------------------
// instanceof
// ----------------------------------------------------------------------------

#[test]
fn instanceof_ancestry() {
    assert_eq!(run_str("class A {} class B extends A {} const b = new B(); `${b instanceof B},${b instanceof A}`"), "true,true");
    assert_eq!(run_str("class A {} class B extends A {} `${new A() instanceof B}`"), "false");
    assert_eq!(
        run_str("class A {} class B extends A {} class C extends B {} const c = new C(); `${c instanceof A},${c instanceof B},${c instanceof C}`"),
        "true,true,true"
    );
    assert_eq!(run_str("class A {} new A() instanceof Object"), "true");
}

// ----------------------------------------------------------------------------
// Static methods
// ----------------------------------------------------------------------------

#[test]
fn static_methods() {
    assert_eq!(run_str("class M { static make(){ return 42; } } M.make()"), "42");
    assert_eq!(
        run_str("class Factory { static create(v){ const f = new Factory(); f.v = v; return f; } get_(){ return this.v; } } Factory.create(9).get_()"),
        "9"
    );
    // static method referencing the class to build instances
    assert_eq!(
        run_str("class Vec { constructor(x){ this.x = x; } static zero(){ return new Vec(0); } } Vec.zero().x"),
        "0"
    );
}

// ----------------------------------------------------------------------------
// Documented C-cluster gaps (asserting actual behavior, not the JS answer)
// ----------------------------------------------------------------------------

#[test]
fn class_field_declarations_unsupported_documented_divergence() {
    // DIVERGENCE (documented, cluster C): class FIELD declarations (`x = 10;`) are
    // not initialized — reading the field yields `undefined`. Use a constructor
    // (see instance_state_via_constructor). Asserting actual behavior.
    assert_eq!(run_str("class C { x = 10; } String(new C().x)"), "undefined"); // JS: 10
    assert_eq!(run_str("class C { x = 10; y = this.x * 2; } String(new C().y)"), "undefined"); // JS: 20
}

#[test]
fn static_field_declarations_unsupported_documented_divergence() {
    // DIVERGENCE (documented, cluster C): STATIC field declarations are not
    // initialized. Static METHODS work (see static_methods).
    assert_eq!(run_str("class C { static count = 5; } String(C.count)"), "undefined"); // JS: 5
}

#[test]
fn accessor_properties_unsupported_documented_divergence() {
    // DIVERGENCE (documented, cluster C): GETTERS/SETTERS are not installed as
    // accessor properties. A `get v()` is treated as an ordinary METHOD, so a bare
    // property read resolves to the function itself (not invoked) — `typeof` is
    // "function". Assigning to a `set v(...)` stores an own data property rather
    // than invoking the setter. Asserting actual behavior.
    assert_eq!(run_str("class T { get val(){ return 99; } } typeof new T().val"), "function"); // JS: "number"
    assert_eq!(run_str("class T { get val(){ return 99; } } new T().val()"), "99"); // works when *called* like a method
    // setter NOT invoked: `_v` stays undefined; the assignment created own `v`.
    assert_eq!(
        run_str("class T { set v(x){ this._v = x * 100; } } const t = new T(); t.v = 5; `${String(t._v)},${t.v}`"),
        "undefined,5" // JS: "500,undefined" (setter runs, no own data prop)
    );
}
