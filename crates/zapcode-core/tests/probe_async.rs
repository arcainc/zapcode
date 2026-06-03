use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, ZapcodeRun};

fn run_str(code: &str) -> String {
    let result = ZapcodeRun::new(code.to_string(), Vec::new(), Vec::new(), ResourceLimits::default())
        .unwrap().run(Vec::new());
    match result {
        Ok(r) => match r.state {
            VmState::Complete(v) => v.to_js_string(&r.heap),
            other => format!("NONCOMPLETE: {other:?}"),
        },
        Err(e) => format!("RUNERR: {e:?}"),
    }
}

#[test]
fn probe() {
    println!("forawait: {}", run_str(r#"
        let s = 0;
        for await (const x of [Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]) { s += x; }
        s
    "#));
    println!("forawait-mixed: {}", run_str(r#"
        let s = 0;
        for await (const x of [1, Promise.resolve(2), 3]) { s += x; }
        s
    "#));
    println!("then-promise: {}", run_str(r#"await Promise.resolve(1).then(x => Promise.resolve(x + 10))"#));
    println!("finally-reject: {}", run_str(r#"
        let r; try { await Promise.reject('boom').finally(() => 'ignored'); r='noerr'; } catch(e){ r='caught:'+e; } r
    "#));
    println!("catch-recover: {}", run_str(r#"await Promise.reject('x').catch(e => 'recovered:' + String(e))"#));
    println!("all-empty: {}", run_str(r#"JSON.stringify(await Promise.all([]))"#));
    println!("resolve-promise: {}", run_str(r#"const p = Promise.resolve(5); (await Promise.resolve(p))"#));
    println!("typeof-promise: {}", run_str(r#"typeof Promise.resolve(1)"#));
    println!("aggregate-exists: {}", run_str(r#"typeof AggregateError"#));
    println!("catch-reason-type: {}", run_str(r#"
        let r; try { await Promise.reject(new Error('msg')).then(x=>x); r='no'; } catch(e){ r=(e instanceof Error)+'|'+e.message; } r
    "#));
    println!("all-order: {}", run_str(r#"JSON.stringify(await Promise.all([Promise.resolve(3), Promise.resolve(1), Promise.resolve(2)]))"#));
    println!("then-onreject: {}", run_str(r#"await Promise.reject('e').then(x => 'ok', e => 'handled:'+e)"#));
    println!("chain-multi: {}", run_str(r#"await Promise.resolve(1).then(x=>x+1).then(x=>x*10).then(x=>x-5)"#));
    println!("await-nonpromise: {}", run_str(r#"await 42"#));
    println!("await-fn-result: {}", run_str(r#"async function f(){ return 7; } await f()"#));
    println!("await-in-loop: {}", run_str(r#"let t=0; for(let i=0;i<3;i++){ t += await Promise.resolve(i); } t"#));
    println!("race-empty-noncomplete: {}", run_str(r#"await Promise.race([])"#));
    println!("any-one-resolve: {}", run_str(r#"await Promise.any([Promise.resolve('a'), Promise.reject('b')])"#));
    println!("allSettled-never-rejects: {}", run_str(r#"
        const r = await Promise.allSettled([Promise.reject('a'), Promise.reject('b')]);
        r.map(x => x.status + ':' + x.reason).join(',')
    "#));
}
