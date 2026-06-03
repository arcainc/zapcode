//! Regression tests for class inheritance (C1: instanceof parent, C2: implicit
//! super forwarding, C3: super.method()/super.prop inside a method body).

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn instanceof_parent_class() {
    assert_eq!(run_str("class A {} class B extends A {} let b = new B(); b instanceof A"), "true");
    assert_eq!(run_str("class A {} class B extends A {} let b = new B(); b instanceof B"), "true");
    assert_eq!(run_str("class A {} class B extends A {} class C extends B {} let c = new C(); c instanceof A"), "true");
    // unrelated class is false
    assert_eq!(run_str("class A {} class B {} let b = new B(); b instanceof A"), "false");
}

#[test]
fn implicit_constructor_forwards_to_super() {
    assert_eq!(run_str("class A { constructor(x){ this.x = x; } } class B extends A {} let b = new B(7); b.x"), "7");
    assert_eq!(run_str("class A { constructor(a,b){ this.sum = a+b; } } class B extends A {} new B(2,3).sum"), "5");
}

#[test]
fn explicit_constructor_super_still_works() {
    assert_eq!(
        run_str("class A { constructor(x){ this.x = x; } } class B extends A { constructor(x){ super(x); this.y = x*2; } } let b = new B(4); b.x + ',' + b.y"),
        "4,8"
    );
}

#[test]
fn inherited_methods_still_work() {
    assert_eq!(run_str("class A { greet(){ return 'hi'; } } class B extends A {} new B().greet()"), "hi");
}

// --- C3: super.method() / super.prop ---

#[test]
fn super_method_returns_value() {
    // The override calls the parent method and uses its return value.
    assert_eq!(
        run_str("class A { g(){ return 1; } } class B extends A { g(){ return super.g() + 10; } } new B().g()"),
        "11"
    );
}

#[test]
fn super_method_only_in_overriding_subclass() {
    // Bare `super.g()` reaching A's g (single level), no extra arithmetic.
    assert_eq!(
        run_str("class A { g(){ return 'A.g'; } } class B extends A { g(){ return super.g(); } } new B().g()"),
        "A.g"
    );
}

#[test]
fn super_method_uses_this() {
    // The parent method itself reads `this`, so the receiver must be the subclass
    // instance (with its own field), not the parent prototype.
    assert_eq!(
        run_str("class A { describe(){ return 'val=' + this.v; } } class B extends A { constructor(v){ super(); this.v = v; } describe(){ return 'B:' + super.describe(); } } new B(7).describe()"),
        "B:val=7"
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
fn super_method_default_field_via_super_call() {
    // Parent method consumes a field set after super() in the child constructor.
    assert_eq!(
        run_str("class Animal { constructor(name){ this.name = name; } speak(){ return this.name + ' makes a noise'; } } class Dog extends Animal { constructor(n){ super(n); } speak(){ return super.speak() + ' (woof)'; } } new Dog('Rex').speak()"),
        "Rex makes a noise (woof)"
    );
}

#[test]
fn super_prop_reads_parent_method_as_value() {
    // `super.g` (no call) yields the parent function value; typeof is 'function'.
    assert_eq!(
        run_str("class A { g(){ return 5; } } class B extends A { g(){ return typeof super.g; } } new B().g()"),
        "function"
    );
}

#[test]
fn super_chain_three_levels() {
    // B overrides A.g; C overrides B.g and calls super.g() (which is B.g, which
    // calls super.g() = A.g). Each level adds to the result.
    assert_eq!(
        run_str(concat!(
            "class A { g(){ return 1; } } ",
            "class B extends A { g(){ return super.g() + 10; } } ",
            "class C extends B { g(){ return super.g() + 100; } } ",
            "new C().g()"
        )),
        "111"
    );
}

#[test]
fn super_method_does_not_break_normal_dispatch() {
    // A sibling subclass without an override still dispatches to the inherited
    // method, while the overriding subclass uses super.method().
    assert_eq!(
        run_str(concat!(
            "class A { g(){ return 'A'; } } ",
            "class B extends A { g(){ return 'B>' + super.g(); } } ",
            "class C extends A {} ",
            "let b = new B().g(); let c = new C().g(); b + ',' + c"
        )),
        "B>A,A"
    );
}

#[test]
fn super_method_coexists_with_instanceof() {
    // instanceof against the parent still holds for a subclass that uses super.
    assert_eq!(
        run_str(concat!(
            "class A { g(){ return 'A'; } } ",
            "class B extends A { g(){ return super.g() + 'B'; } } ",
            "let b = new B(); let v = b.g(); let isA = b instanceof A; v + ',' + isA"
        )),
        "AB,true"
    );
}
