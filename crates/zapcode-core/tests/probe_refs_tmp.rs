use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default()).unwrap().run(Vec::new()).unwrap();
    match result.state {
        VmState::Complete(v) => v.to_js_string(&result.heap),
        other => panic!("expected completion for `{code}`, got {other:?}"),
    }
}

#[test]
fn probe() {
    let cases = [
        // WeakMap
        "const wm=new WeakMap(); const k={}; wm.set(k,5); wm.get(k)",
        "const wm=new WeakMap(); const k={}; wm.set(k,5); wm.has(k)",
        "const wm=new WeakMap(); const k={}; wm.set(k,5); wm.has({})",
        // splice returns refs to removed objects
        "const inner={n:1}; const a=[inner]; const removed=a.splice(0,1); removed[0]===inner",
        // sort with object refs keeps identity
        "const x={v:2}; const y={v:1}; const a=[x,y]; a.sort((p,q)=>p.v-q.v); a[0]===y",
        // find returns ref
        "const inner={n:1}; const a=[inner]; a.find(e=>e.n===1)===inner",
        // filter returns refs
        "const inner={n:1}; const a=[inner]; a.filter(()=>true)[0]===inner",
        // map callback can mutate elements
        "const a=[{n:1},{n:2}]; a.map(e=>{e.n+=100;return e}); JSON.stringify(a)",
        // forEach mutate
        "const a=[{n:1}]; a.forEach(e=>e.n=9); a[0].n",
        // reduce accumulator ref
        "const a=[1,2,3]; const r=a.reduce((acc,x)=>{acc.push(x*2);return acc},[]); JSON.stringify(r)",
        // array destructuring aliases
        "const inner={n:1}; const a=[inner]; const [first]=a; first.n=7; a[0].n",
        // object destructuring aliases nested object
        "const o={inner:{n:1}}; const {inner}=o; inner.n=7; o.inner.n",
        // rest in destructuring shallow copies refs
        "const inner={n:1}; const a=[0,inner]; const [,...rest]=a; rest[0]===inner",
        // default param array is fresh each call
        "const f=(a=[])=>{a.push(1);return a.length}; JSON.stringify([f(),f()])",
        // closure captures shared array
        "const make=()=>{const a=[];return {add:(x)=>a.push(x),get:()=>a}}; const m=make(); m.add(1); m.add(2); JSON.stringify(m.get())",
        // two closures share state
        "let count; const inc=(()=>{const o={n:0};return ()=>++o.n})(); JSON.stringify([inc(),inc(),inc()])",
        // Map iteration yields same handles
        "const inner={n:1}; const m=new Map(); m.set('a',inner); let same=false; for(const [k,v] of m){same=(v===inner)} same",
        // Object as map key distinct from clone
        "const k={}; const m=new Map(); m.set(k,1); const c=structuredClone(k); m.has(c)",
        // structuredClone of nested array of objects independence
        "const a=[{n:1}]; const c=structuredClone(a); c[0].n=99; a[0].n",
        // structuredClone preserves internal shared ref as distinct copies? (shared sibling becomes... structuredClone preserves identity!)
        "const shared={n:1}; const o={a:shared,b:shared}; const c=structuredClone(o); c.a===c.b",
        // but original still shares
        "const shared={n:1}; const o={a:shared,b:shared}; o.a===o.b",
        // clone shared then mutate one path affects both in clone
        "const shared={n:1}; const o={a:shared,b:shared}; const c=structuredClone(o); c.a.n=42; c.b.n",
        // nested push via getter chain
        "const o={list:[]}; o.list.push({id:1}); o.list[0].id",
        // array holding map
        "const m=new Map(); const a=[m]; a[0].set('x',1); m.get('x')",
        // delete then identity
        "const o={a:1}; delete o.a; ('a' in o)",
        // shared object survives array reverse
        "const x={v:1}; const a=[x,{v:2}]; a.reverse(); a[1]===x",
    ];
    for c in cases { println!("CASE: {c}\n  => {}", run_str(c)); }
}
