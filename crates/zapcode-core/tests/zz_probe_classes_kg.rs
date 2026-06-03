use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};
fn run_str(code: &str) -> String {
    match ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default()) {
        Ok(r) => match r.run(Vec::new()) {
            Ok(res) => match res.state {
                VmState::Complete(v) => v.to_js_string(&res.heap),
                other => format!("NONCOMPLETE: {other:?}"),
            },
            Err(e) => format!("RUNERR: {e:?}"),
        },
        Err(e) => format!("COMPILEERR: {e:?}"),
    }
}
#[test]
fn probe() {
    let cases: &[(&str, &str)] = &[
        ("method default param both", "class C { scale(v, f = 2){ return v * f; } } `${new C().scale(5)},${new C().scale(5,3)}`"),
        ("method default param single call", "class C { scale(v, f = 2){ return v * f; } } String(new C().scale(5))"),
        ("method default param explicit two", "class C { scale(v, f = 2){ return v * f; } } String(new C().scale(5,3))"),
        ("method default param via var", "class C { scale(v, f = 2){ return v * f; } } const c = new C(); String(c.scale(5))"),
        ("template with new in interp", "class C { scale(v){ return v*2; } } `${new C().scale(5)}`"),
        ("template two new interps", "class C { scale(v){ return v*2; } } `${new C().scale(5)},${new C().scale(3)}`"),
        ("plain method no default in template", "class C { scale(v, f){ return v*f; } } `${new C().scale(5,2)}`"),
    ];
    for (label, code) in cases {
        println!("[KG] [{label}] => {}", run_str(code));
    }
}
