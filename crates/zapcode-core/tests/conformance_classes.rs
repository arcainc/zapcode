//! Conformance breadth: classes & inheritance (round 1).
//!
//! A language-test-suite-style sweep of the fully-supported class surface:
//! constructors (explicit/implicit, defaults, rest, returns), instance methods,
//! `this` binding & chaining, method/getter dispatch, `extends`, `super(...)`
//! forwarding (explicit + implicit), `super.method()` / `super.prop` (multi-level),
//! `instanceof` across the ancestor chain, method overriding, static methods,
//! class expressions (named + anonymous), and integration with closures, JSON,
//! and error handling.
//!
//! KNOWN, DOCUMENTED DIVERGENCES (asserted at zapcode's ACTUAL behavior, with the
//! real-Node answer noted in a comment — never asserting the JS answer where it
//! diverges, per the suite's GREEN guarantee):
//!   * `typeof Class` is "object" (JS: "function").
//!   * `Class.name` / `instance.constructor` are not installed (JS: "Name" / the class).
//!   * static methods are NOT inherited by subclasses (JS: they are).
//!   * methods are per-instance, not shared on a prototype object
//!     (`a.m !== b.m`; JS: `===`).
//!   * class FIELD declarations (`x = …;`), STATIC fields, and accessor
//!     GET/SET semantics are not installed; a `get x()` is an ordinary method.
//!   * `#private` fields and computed method names `[k](){}` are unsupported syntax.
//!   * a class expression *returned through a function* and then `new`'d yields a
//!     plain object (so its method results coerce to "[object Object]"); we exercise
//!     class expressions assigned directly, which work, and skip the escaping form.
//! The supported idiom for instance state is constructor assignment; arrow
//! callbacks that run synchronously inside a method capture `this` correctly.

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

/// Run and expect a thrown/runtime error (returns the Debug string of the error).
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
// 1. Construction & instance methods
// ============================================================================

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
fn empty_class_constructs() {
    assert_eq!(run_str("class C {} typeof new C()"), "object");
    assert_eq!(run_str("class C {} const c = new C(); c.x = 1; String(c.x)"), "1");
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
fn constructor_default_parameters() {
    assert_eq!(
        run_str("class C { constructor(x = 10){ this.x = x; } } const a = new C(); const b = new C(3); `${a.x},${b.x}`"),
        "10,3"
    );
    // omitted required-looking param is just undefined (no default)
    assert_eq!(
        run_str("class C { constructor(x){ this.x = x; } } String(new C().x)"),
        "undefined"
    );
}

#[test]
fn constructor_rest_parameters() {
    assert_eq!(
        run_str("class C { constructor(...xs){ this.total = xs.reduce((a,b)=>a+b,0); } } String(new C(1,2,3,4).total)"),
        "10"
    );
}

#[test]
fn method_rest_parameters() {
    assert_eq!(
        run_str("class C { sum(...xs){ return xs.reduce((a,b)=>a+b,0); } } String(new C().sum(1,2,3,4))"),
        "10"
    );
}

#[test]
fn method_default_parameters() {
    assert_eq!(
        run_str("class C { scale(v, f = 2){ return v * f; } } const c = new C(); `${c.scale(5)},${c.scale(5,3)}`"),
        "10,15"
    );
}

#[test]
fn constructor_empty_return_yields_this() {
    assert_eq!(
        run_str("class C { constructor(){ this.x = 1; return; } } String(new C().x)"),
        "1"
    );
}

#[test]
fn constructor_returning_object_overrides_this() {
    // A constructor that returns an object replaces the freshly-created `this`.
    assert_eq!(
        run_str("class C { constructor(){ return { y: 99 }; } } String(new C().y)"),
        "99"
    );
}

#[test]
fn multiple_methods_one_instance() {
    assert_eq!(
        run_str("class C { constructor(){ this.v = 1; } a(){ return this.v; } b(){ return this.v * 10; } } const c = new C(); String(c.a() + c.b())"),
        "11"
    );
}

#[test]
fn method_calls_sibling_method_via_this() {
    assert_eq!(
        run_str("class C { a(){ return this.b() + 1; } b(){ return 10; } } String(new C().a())"),
        "11"
    );
}

#[test]
fn method_returns_new_instance_of_same_class() {
    assert_eq!(
        run_str("class Node { constructor(v){ this.v = v; } next(){ return new Node(this.v + 1); } } String(new Node(1).next().next().v)"),
        "3"
    );
}

// ============================================================================
// 2. `this` binding & method chaining
// ============================================================================

#[test]
fn this_chaining_methods() {
    assert_eq!(
        run_str("class Counter { constructor(){ this.n = 0; } inc(){ this.n++; return this; } } const c = new Counter(); c.inc().inc().inc().n"),
        "3"
    );
}

#[test]
fn instances_have_independent_state() {
    assert_eq!(
        run_str("class C { constructor(){ this.n = 0; } inc(){ this.n++; return this; } } const a = new C(); const b = new C(); a.inc().inc(); b.inc(); `${a.n},${b.n}`"),
        "2,1"
    );
}

#[test]
fn method_invoked_through_receiver_sees_this() {
    assert_eq!(
        run_str("class C { constructor(){ this.n = 7; } m(){ return this.n; } } const inst = new C(); String(inst.m())"),
        "7"
    );
}

#[test]
fn extracted_method_is_a_function_value() {
    // Pulling the method off the instance yields a function value.
    assert_eq!(
        run_str("class C { constructor(){ this.n = 3; } m(){ return this.n; } } const c = new C(); const f = c.m; typeof f"),
        "function"
    );
}

#[test]
fn fluent_builder_pattern() {
    assert_eq!(
        run_str("class Builder { constructor(){ this.parts = []; } add(p){ this.parts.push(p); return this; } build(){ return this.parts.join('-'); } } new Builder().add('a').add('b').add('c').build()"),
        "a-b-c"
    );
}

// ============================================================================
// 3. Arrow callbacks inside methods capture `this`
// ============================================================================

#[test]
fn arrow_inside_method_captures_this_synchronously() {
    assert_eq!(
        run_str("class C { constructor(){ this.n = 5; } run(){ const f = () => this.n; return f(); } } String(new C().run())"),
        "5"
    );
}

#[test]
fn arrow_in_array_map_captures_this() {
    assert_eq!(
        run_str("class C { constructor(){ this.k = 2; } go(arr){ return arr.map(x => x * this.k); } } new C().go([1,2,3]).join(',')"),
        "2,4,6"
    );
}

#[test]
fn arrow_in_filter_and_reduce_captures_this() {
    assert_eq!(
        run_str("class C { constructor(){ this.min = 2; } keep(arr){ return arr.filter(x => x >= this.min); } } new C().keep([1,2,3,1,4]).join(',')"),
        "2,3,4"
    );
    assert_eq!(
        run_str("class C { constructor(){ this.base = 100; } total(arr){ return arr.reduce((acc, x) => acc + x, this.base); } } String(new C().total([1,2,3]))"),
        "106"
    );
}

// ============================================================================
// 4. Getters used as methods (documented: get x() is an ordinary method)
// ============================================================================

#[test]
fn getter_is_treated_as_method() {
    // DIVERGENCE (documented): `get v()` installs an ordinary method, not an
    // accessor. A bare read resolves to the function value; *calling* it works.
    assert_eq!(run_str("class T { get val(){ return 99; } } typeof new T().val"), "function"); // JS: "number"
    assert_eq!(run_str("class T { get val(){ return 99; } } new T().val()"), "99"); // works when called like a method
}

#[test]
fn explicit_getter_method_idiom_works() {
    // The portable idiom — an explicit accessor method — works as expected.
    assert_eq!(
        run_str("class C { constructor(){ this._v = 7; } getV(){ return this._v; } } String(new C().getV())"),
        "7"
    );
}

// ============================================================================
// 5. Class expressions
// ============================================================================

#[test]
fn anonymous_class_expression() {
    assert_eq!(run_str("const C = class { f(){ return 7; } }; new C().f()"), "7");
}

#[test]
fn named_class_expression() {
    assert_eq!(run_str("const Named = class Inner { val(){ return 11; } }; new Named().val()"), "11");
}

#[test]
fn class_expression_with_constructor_and_state() {
    assert_eq!(
        run_str("const Box = class { constructor(v){ this.v = v; } get_(){ return this.v; } }; new Box(42).get_()"),
        "42"
    );
}

#[test]
fn class_expression_extending_another_expression() {
    assert_eq!(
        run_str("const base = class { f(){ return 'b'; } }; class D extends base { f(){ return super.f() + 'd'; } } new D().f()"),
        "bd"
    );
}

#[test]
fn class_expression_instanceof_chain() {
    assert_eq!(
        run_str("const C = class {}; const D = class extends C {}; String(new D() instanceof C)"),
        "true"
    );
}

#[test]
fn class_assigned_to_alias_var_constructs_and_instanceof() {
    assert_eq!(run_str("class A {} const B = A; String(new B() instanceof A)"), "true");
}

// ============================================================================
// 6. Inheritance — method resolution
// ============================================================================

#[test]
fn inheritance_inherits_methods() {
    assert_eq!(run_str("class A { f(){ return 1; } } class B extends A {} new B().f()"), "1");
    assert_eq!(
        run_str("class Animal { speak(){ return 'generic'; } } class Dog extends Animal {} new Dog().speak()"),
        "generic"
    );
}

#[test]
fn inheritance_three_levels_deep() {
    assert_eq!(
        run_str("class A { f(){ return 'a'; } } class B extends A {} class C extends B {} new C().f()"),
        "a"
    );
}

#[test]
fn subclass_adds_its_own_method() {
    assert_eq!(
        run_str("class A { f(){ return 1; } } class B extends A { g(){ return 2; } } const b = new B(); String(b.f() + b.g())"),
        "3"
    );
}

#[test]
fn override_at_each_level() {
    assert_eq!(run_str("class A { f(){ return 1; } } class B extends A { f(){ return 2; } } class C extends B {} new C().f()"), "2");
    assert_eq!(run_str("class A { f(){ return 1; } } class B extends A {} class C extends B { f(){ return 3; } } new C().f()"), "3");
}

#[test]
fn override_does_not_affect_parent_instance() {
    assert_eq!(
        run_str("class A { f(){ return 'pa'; } } class B extends A { f(){ return 'ch'; } } new A().f() + ',' + new B().f()"),
        "pa,ch"
    );
}

// ============================================================================
// 7. super(...) constructor forwarding
// ============================================================================

#[test]
fn super_constructor_explicit() {
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A { constructor(){ super(5); } } new B().x"),
        "5"
    );
}

#[test]
fn super_constructor_implicit_forwards_args() {
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A {} new B(7).x"),
        "7"
    );
    assert_eq!(
        run_str("class A { constructor(a,b){ this.sum = a + b; } } class B extends A {} new B(2,3).sum"),
        "5"
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
fn super_constructor_ordering_of_side_effects() {
    // Parent ctor runs first (appends 'A'), then child body (appends 'B').
    assert_eq!(
        run_str("class A { constructor(){ this.list = []; this.list.push('A'); } } class B extends A { constructor(){ super(); this.list.push('B'); } } new B().list.join(',')"),
        "A,B"
    );
}

#[test]
fn super_constructor_string_accumulation_order() {
    assert_eq!(
        run_str("class A { constructor(){ this.seq = 'a'; } } class B extends A { constructor(){ super(); this.seq += 'b'; } } new B().seq"),
        "ab"
    );
}

#[test]
fn super_constructor_default_field_consumed_by_parent_method() {
    assert_eq!(
        run_str("class Animal { constructor(name){ this.name = name; } speak(){ return this.name + ' makes a noise'; } } class Dog extends Animal { constructor(n){ super(n); } speak(){ return super.speak() + ' (woof)'; } } new Dog('Rex').speak()"),
        "Rex makes a noise (woof)"
    );
}

// ============================================================================
// 8. super.method() / super.prop inside method bodies
// ============================================================================

#[test]
fn super_method_call_single_level() {
    assert_eq!(
        run_str("class A { g(){ return 10; } } class B extends A { g(){ return super.g() + 5; } } new B().g()"),
        "15"
    );
}

#[test]
fn super_method_returns_parent_value_unmodified() {
    assert_eq!(
        run_str("class A { g(){ return 'A.g'; } } class B extends A { g(){ return super.g(); } } new B().g()"),
        "A.g"
    );
}

#[test]
fn super_method_with_arguments() {
    assert_eq!(
        run_str("class A { add(a,b){ return a + b; } } class B extends A { add(a,b){ return super.add(a,b) * 2; } } new B().add(3,4)"),
        "14"
    );
}

#[test]
fn super_method_called_twice_with_arg() {
    assert_eq!(
        run_str("class A { f(a){ return a; } } class B extends A { f(a){ return super.f(a) + super.f(a); } } String(new B().f(5))"),
        "10"
    );
}

#[test]
fn super_method_reads_this_set_after_super_call() {
    assert_eq!(
        run_str("class A { describe(){ return 'A:' + this.tag; } } class B extends A { constructor(){ super(); this.tag = 'b'; } describe(){ return super.describe() + '|B'; } } new B().describe()"),
        "A:b|B"
    );
}

#[test]
fn super_method_uses_subclass_this_identity() {
    assert_eq!(
        run_str("class A { who(){ return this.tag; } } class B extends A { constructor(){ super(); this.tag = 'B'; } who(){ return super.who(); } } new B().who()"),
        "B"
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
fn super_chain_string_accumulation() {
    assert_eq!(
        run_str("class A { f(){ return 'A'; } } class B extends A { f(){ return 'B' + super.f(); } } class C extends B { f(){ return 'C' + super.f(); } } new C().f()"),
        "CBA"
    );
}

#[test]
fn super_chain_four_levels() {
    assert_eq!(
        run_str("class A{f(){return 'a';}} class B extends A{f(){return super.f()+'b';}} class C extends B{f(){return super.f()+'c';}} class D extends C{f(){return super.f()+'d';}} new D().f()"),
        "abcd"
    );
}

#[test]
fn super_prop_without_call_is_function_value() {
    // `super.g` (no call) yields the parent function value.
    assert_eq!(
        run_str("class A { g(){ return 5; } } class B extends A { h(){ return typeof super.g; } } new B().h()"),
        "function"
    );
}

#[test]
fn super_method_coexists_with_unoverridden_sibling() {
    assert_eq!(
        run_str("class A { g(){ return 'A'; } } class B extends A { g(){ return 'B>' + super.g(); } } class C extends A {} let b = new B().g(); let c = new C().g(); b + ',' + c"),
        "B>A,A"
    );
}

// ============================================================================
// 9. instanceof — ancestor chain
// ============================================================================

#[test]
fn instanceof_direct_class() {
    assert_eq!(run_str("class A {} const a = new A(); String(a instanceof A)"), "true");
}

#[test]
fn instanceof_parent_and_self() {
    assert_eq!(
        run_str("class A {} class B extends A {} const b = new B(); `${b instanceof B},${b instanceof A}`"),
        "true,true"
    );
}

#[test]
fn instanceof_three_level_chain() {
    assert_eq!(
        run_str("class A {} class B extends A {} class C extends B {} const c = new C(); `${c instanceof A},${c instanceof B},${c instanceof C}`"),
        "true,true,true"
    );
}

#[test]
fn instanceof_negative_cases() {
    assert_eq!(run_str("class A {} class B extends A {} `${new A() instanceof B}`"), "false");
    assert_eq!(run_str("class A {} class B {} const b = new B(); String(b instanceof A)"), "false");
    assert_eq!(run_str("class A {} class B {} const a = new A(); `${a instanceof A},${a instanceof B}`"), "true,false");
}

#[test]
fn instanceof_object_for_all_instances() {
    assert_eq!(run_str("class A {} new A() instanceof Object"), "true");
    assert_eq!(run_str("class A {} class B extends A {} new B() instanceof Object"), "true");
}

#[test]
fn instanceof_non_object_lhs_is_false() {
    assert_eq!(run_str("class A {} String(1 instanceof A)"), "false");
    assert_eq!(run_str("class A {} String({} instanceof A)"), "false");
}

#[test]
fn instanceof_survives_method_using_super() {
    assert_eq!(
        run_str("class A { g(){ return 'A'; } } class B extends A { g(){ return super.g() + 'B'; } } const b = new B(); `${b.g()},${b instanceof A}`"),
        "AB,true"
    );
}

// ============================================================================
// 10. Static methods
// ============================================================================

#[test]
fn static_method_basic() {
    assert_eq!(run_str("class M { static make(){ return 42; } } M.make()"), "42");
}

#[test]
fn static_method_with_params() {
    assert_eq!(run_str("class M { static add(a,b){ return a + b; } } String(M.add(3,4))"), "7");
}

#[test]
fn static_method_calls_another_static_via_this() {
    assert_eq!(
        run_str("class C { static a(){ return 1; } static b(){ return this.a() + 2; } } String(C.b())"),
        "3"
    );
}

#[test]
fn static_method_references_class_to_build_instances() {
    assert_eq!(
        run_str("class Vec { constructor(x){ this.x = x; } static zero(){ return new Vec(0); } } Vec.zero().x"),
        "0"
    );
    assert_eq!(
        run_str("class Factory { static create(v){ const f = new Factory(); f.v = v; return f; } get_(){ return this.v; } } Factory.create(9).get_()"),
        "9"
    );
}

#[test]
fn static_new_this_refers_to_class() {
    assert_eq!(
        run_str("class A { constructor(){ this.k = 1; } static build(){ return new this(); } } String(A.build().k)"),
        "1"
    );
    assert_eq!(
        run_str("class C { static make(){ return new C(); } } C.make() instanceof C ? 'yes' : 'no'"),
        "yes"
    );
}

#[test]
fn static_method_returns_object_literal() {
    assert_eq!(run_str("class C { static cfg(){ return { a: 1, b: 2 }; } } JSON.stringify(C.cfg())"), r#"{"a":1,"b":2}"#);
}

#[test]
fn static_factory_returns_fluent_builder() {
    assert_eq!(
        run_str("class Builder { constructor(){ this.parts = []; } add(p){ this.parts.push(p); return this; } build(){ return this.parts.join('-'); } static start(){ return new Builder(); } } Builder.start().add('a').add('b').build()"),
        "a-b"
    );
}

#[test]
fn subclass_static_can_reference_parent_static_explicitly() {
    // Static methods are not auto-inherited (see divergence test), but a subclass
    // static can still reach the parent's static by naming the parent class.
    assert_eq!(
        run_str("class A { static f(){ return 1; } } class B extends A { static g(){ return A.f() + 1; } } String(B.g())"),
        "2"
    );
}

#[test]
fn static_state_assigned_externally() {
    assert_eq!(run_str("class C {} C.x = 5; String(C.x)"), "5");
}

// ============================================================================
// 11. Integration: classes with closures, JSON, errors, control flow
// ============================================================================

#[test]
fn instance_json_stringify_serializes_own_fields() {
    assert_eq!(
        run_str("class C { constructor(){ this.a = 1; this.b = 2; } } JSON.stringify(new C())"),
        r#"{"a":1,"b":2}"#
    );
}

#[test]
fn array_of_instances_mapped() {
    assert_eq!(
        run_str("class P { constructor(n){ this.n = n; } twice(){ return this.n * 2; } } [1,2,3].map(x => new P(x).twice()).join(',')"),
        "2,4,6"
    );
}

#[test]
fn instances_in_collections() {
    assert_eq!(
        run_str("class Item { constructor(id){ this.id = id; } } const arr = [new Item(1), new Item(2), new Item(3)]; arr.map(i => i.id).reduce((a,b)=>a+b,0)"),
        "6"
    );
}

#[test]
fn constructor_throwing_is_catchable() {
    assert_eq!(
        run_str("class C { constructor(){ throw new Error('boom'); } } try { new C(); 'no' } catch(e){ e.message }"),
        "boom"
    );
}

#[test]
fn method_throwing_is_catchable() {
    assert_eq!(
        run_str("class C { boom(){ throw new Error('x'); } } const c = new C(); try { c.boom(); 'no' } catch(e){ e.message }"),
        "x"
    );
}

#[test]
fn class_declared_in_block_scope() {
    assert_eq!(run_str("{ class Local { f(){ return 3; } } } 'ok'"), "ok");
}

#[test]
fn polymorphism_over_a_list() {
    // Classic OO polymorphism: a heterogeneous list dispatched via overridden method.
    assert_eq!(
        run_str(concat!(
            "class Shape { area(){ return 0; } } ",
            "class Square extends Shape { constructor(s){ super(); this.s = s; } area(){ return this.s * this.s; } } ",
            "class Rect extends Shape { constructor(w,h){ super(); this.w = w; this.h = h; } area(){ return this.w * this.h; } } ",
            "const shapes = [new Square(2), new Rect(3, 4), new Shape()]; ",
            "shapes.map(s => s.area()).join(',')"
        )),
        "4,12,0"
    );
}

#[test]
fn linked_list_via_classes() {
    assert_eq!(
        run_str(concat!(
            "class Node { constructor(v){ this.v = v; this.next = null; } } ",
            "const a = new Node(1); a.next = new Node(2); a.next.next = new Node(3); ",
            "let sum = 0; let cur = a; while (cur) { sum += cur.v; cur = cur.next; } String(sum)"
        )),
        "6"
    );
}

#[test]
fn stack_implemented_with_class() {
    assert_eq!(
        run_str(concat!(
            "class Stack { constructor(){ this.items = []; } push(x){ this.items.push(x); return this; } pop(){ return this.items.pop(); } get size(){ return this.items.length; } } ",
            "const s = new Stack(); s.push(1).push(2).push(3); `${s.pop()},${s.pop()},${s.items.length}`"
        )),
        "3,2,1"
    );
}

// ============================================================================
// 12. Method overriding (named, ordered checks)
// ============================================================================

#[test]
fn method_overriding_simple() {
    assert_eq!(
        run_str("class A { name(){ return 'A'; } } class B extends A { name(){ return 'B'; } } new B().name()"),
        "B"
    );
}

#[test]
fn method_overriding_uses_own_field() {
    assert_eq!(
        run_str("class Shape { area(){ return 0; } } class Square extends Shape { constructor(s){ super(); this.s = s; } area(){ return this.s * this.s; } } new Square(5).area()"),
        "25"
    );
}

#[test]
fn two_unrelated_classes_share_a_method_name() {
    assert_eq!(
        run_str("class A { id(){ return 'A'; } } class B { id(){ return 'B'; } } new A().id() + new B().id()"),
        "AB"
    );
}

// ============================================================================
// 13. toString coercion hook on instances
// ============================================================================

#[test]
fn to_string_hook_on_instance() {
    assert_eq!(
        run_str("class P { constructor(n){ this.n = n; } toString(){ return 'P(' + this.n + ')'; } } '' + new P(3)"),
        "P(3)"
    );
}

#[test]
fn default_to_string_is_object_object() {
    assert_eq!(run_str("class C {} ('' + new C())"), "[object Object]");
}

#[test]
fn to_string_hook_in_template_literal() {
    assert_eq!(
        run_str("class Money { constructor(c){ this.c = c; } toString(){ return '$' + this.c; } } `cost: ${new Money(5)}`"),
        "cost: $5"
    );
}

// ============================================================================
// 14. Documented divergences — asserted at ACTUAL behavior, JS answer in comment
// ============================================================================

#[test]
fn typeof_class_is_object_documented_divergence() {
    // DIVERGENCE (documented): a class value's typeof is "object" here.
    assert_eq!(run_str("class C {} typeof C"), "object"); // JS: "function"
}

#[test]
fn class_name_property_absent_documented_divergence() {
    // DIVERGENCE (documented): `Class.name` is not installed.
    assert_eq!(run_str("class Foo {} String(Foo.name)"), "undefined"); // JS: "Foo"
}

#[test]
fn instance_constructor_property_absent_documented_divergence() {
    // DIVERGENCE (documented): instances do not carry a `.constructor` back-link.
    assert_eq!(run_str("class Foo {} String(new Foo().constructor === Foo)"), "false"); // JS: true
    assert_eq!(run_str("class Foo {} typeof new Foo().constructor"), "undefined"); // JS: "function"
}

#[test]
fn methods_are_per_instance_not_shared_documented_divergence() {
    // DIVERGENCE (documented): each instance gets its own bound method value, so
    // two instances' method references are not identical.
    assert_eq!(run_str("class C { m(){ return 1; } } const a = new C(); const b = new C(); String(a.m === b.m)"), "false"); // JS: true
}

#[test]
fn static_methods_not_inherited_documented_divergence() {
    // DIVERGENCE (documented): static methods are not inherited by subclasses;
    // reach the parent static by naming the parent class instead.
    assert_eq!(
        run_err("class A { static f(){ return 9; } } class B extends A {} B.f()"),
        // JS: returns 9 (statics inherit through the constructor's prototype chain)
        r#"TypeError("undefined is not a function")"#
    );
}

#[test]
fn class_field_declarations_unsupported_documented_divergence() {
    // DIVERGENCE (documented, cluster C): class FIELD declarations (`x = 10;`) are
    // not initialized — reading the field yields `undefined`. Use a constructor.
    assert_eq!(run_str("class C { x = 10; } String(new C().x)"), "undefined"); // JS: 10
    assert_eq!(run_str("class C { x = 10; y = this.x * 2; } String(new C().y)"), "undefined"); // JS: 20
    assert_eq!(run_str("class C { x; } String(new C().x)"), "undefined"); // JS: undefined (here coincidentally matches)
}

#[test]
fn static_field_declarations_unsupported_documented_divergence() {
    // DIVERGENCE (documented, cluster C): STATIC field declarations are not
    // initialized. Static METHODS work (see static_method_basic).
    assert_eq!(run_str("class C { static count = 5; } String(C.count)"), "undefined"); // JS: 5
    assert_eq!(run_str("class C { static count = 0; } String(C.count)"), "undefined"); // JS: 0
}

#[test]
fn setter_not_invoked_documented_divergence() {
    // DIVERGENCE (documented, cluster C): a `set v(...)` is not installed as an
    // accessor; assigning `t.v = 5` stores an own data property and the setter body
    // never runs (so `_v` stays undefined).
    assert_eq!(
        run_str("class T { set v(x){ this._v = x * 100; } } const t = new T(); t.v = 5; `${String(t._v)},${t.v}`"),
        "undefined,5" // JS: "500,undefined"
    );
}

#[test]
fn private_fields_are_unsupported_syntax() {
    // DIVERGENCE (documented): `#private` fields are unsupported syntax.
    let e = run_err("class C { #x = 5; getX(){ return this.#x; } } new C().getX()");
    assert!(e.contains("private fields are not supported"), "got: {e}"); // JS: 5
}

#[test]
fn computed_method_names_unsupported() {
    // Computed method names `[k](){}` do not install a callable method; the lookup
    // misses and calling it is a TypeError. (JS: defines a method named by `k`.)
    assert_eq!(
        run_err("const k = 'foo'; class C { [k](){ return 7; } } new C().foo()"),
        r#"TypeError("undefined is not a function")"#
    );
}
