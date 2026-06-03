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
    let cases = [
        // mixed array-in-object for-of
        "const out=[]; for(const {tags:[first]} of [{tags:['a','b']},{tags:['c']}]) out.push(first); out.join(',')",
        // mixed array-in-object param
        "function f({coords:[x,y]}){return `${x},${y}`} f({coords:[3,4]})",
        "const f=({items:[first,...rest]})=>`${first}|${rest.join(',')}`; f({items:[1,2,3]})",
        // object-in-array param
        "function f([{a},{b}]){return a+b} f([{a:1},{b:2}])",
        "const f=([{name}])=>name; f([{name:'x'}])",
        // deep object with rest+default
        "const {a:{b=5,...rest}}={a:{b:1,c:2,d:3}}; JSON.stringify([b,rest])",
        // realistic record
        "const record={id:42,profile:{firstName:'Ada',lastName:'L'},role:'admin',tags:['x']}; const {id,profile:{firstName:fn},...rest}=record; JSON.stringify([id,fn,rest])",
        // assignment forms - which compile?
        "let a=1,b=2; [a,b]=[b,a]; a+','+b",
        "let a,b; ({a,b}={a:1,b:2}); a+b",
        "const o={}; ({x:o.p}={x:7}); o.p",
        "let arr=[]; [arr[0],arr[1]]=[1,2]; JSON.stringify(arr)",
    ];
    for c in cases {
        println!("CASE: {c}\n  => {}", run_str(c));
    }
}
