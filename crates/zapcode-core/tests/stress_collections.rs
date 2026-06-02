//! Regression tests for collection divergences (cluster H): sort in place (H1),
//! Map spread (H2), new Map(map)/new Set(string) (H4/H9), flat depth (H5),
//! NaN in Set/Map/includes (H7), includes/indexOf fromIndex (H8).

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
        VmState::Complete(v) => v.to_js_string(),
        other => panic!("expected completion, got {other:?}"),
    }
}

#[test]
fn sort_mutates_in_place() {
    assert_eq!(run_str("const a=[3,1,2]; a.sort((x,y)=>x-y); a.join(',')"), "1,2,3");
    assert_eq!(run_str("const a=[3,1,2]; a.sort(); a.join(',')"), "1,2,3");
    // Multi-key comparator (regression of the O1 string-relational bug).
    assert_eq!(
        run_str(
            "const a=[{p:2,id:'b'},{p:1,id:'z'},{p:1,id:'a'}]; a.sort((x,y)=>x.p!==y.p?x.p-y.p:(x.id<y.id?-1:1)); a.map(o=>o.id).join(',')"
        ),
        "a,z,b"
    );
}

#[test]
fn spread_map_and_set() {
    assert_eq!(
        run_str("const m=new Map([['a',1],['b',2]]); [...m].map(e=>e[0]+'='+e[1]).join(',')"),
        "a=1,b=2"
    );
    assert_eq!(run_str("[...new Set([1,1,2,3])].join(',')"), "1,2,3");
}

#[test]
fn map_and_set_from_iterables() {
    assert_eq!(run_str("const m=new Map([['a',1]]); new Map(m).get('a')"), "1");
    assert_eq!(run_str("new Set('aab').size"), "2");
    assert_eq!(run_str("[...new Set('aab')].join('')"), "ab");
}

#[test]
fn flat_depth() {
    assert_eq!(
        run_str("[1,[2,[3]]].flat(2).every(x=>typeof x==='number')"),
        "true"
    );
    assert_eq!(run_str("[1,[2,[3,[4]]]].flat(Infinity).length"), "4");
    // Default depth is 1: the inner array survives.
    assert_eq!(run_str("Array.isArray([1,[2,[3]]].flat(1)[2])"), "true");
}

#[test]
fn nan_same_value_zero() {
    assert_eq!(run_str("new Set([NaN,NaN]).size"), "1");
    assert_eq!(run_str("const m=new Map(); m.set(NaN,9); m.get(NaN)"), "9");
    assert_eq!(run_str("[NaN].includes(NaN)"), "true");
}

#[test]
fn includes_indexof_from_index() {
    assert_eq!(run_str("[1,2,3].includes(1,1)"), "false");
    assert_eq!(run_str("[1,2,3,1].indexOf(1,-2)"), "3");
    assert_eq!(run_str("[1,2,3].indexOf(2,1)"), "1");
}
