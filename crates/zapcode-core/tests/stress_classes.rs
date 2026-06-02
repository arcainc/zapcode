//! Regression tests for class inheritance (C1: instanceof parent, C2: implicit
//! super forwarding). C3 (super.method) remains a follow-up.

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
